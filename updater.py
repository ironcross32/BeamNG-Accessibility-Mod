# updater.py
# GitHub-only auto-updater for BEAM.
#
# The whole product ships as ONE release asset -- build.py's bundle_output()
# zips beamtel.exe, README.md and bng_screenreader_mod.zip together, and
# .github/workflows/release.yml publishes exactly that on a v* tag. So there is
# nothing to negotiate about what an update is: fetch that asset, put its
# contents where the running program lives, and offer the inner mod zip to
# BeamNG afterwards.
#
# Two facts shape everything below.
#
#   * The running exe cannot replace itself. Nuitka builds beamtel as a onefile
#     binary and Windows holds it open for as long as the process lives, so the
#     swap has to happen after we exit -- hence a detached .cmd helper that
#     waits on our PID, copies the staged files over, and starts the new exe.
#     That restart is why the mod install cannot happen in the same run: the
#     zip we want to install is the one being copied in.
#
#   * The two halves of this project (the exe and the Lua/JS mod) go out of
#     step trivially, and a skew is silent -- the mod is a live directory in the
#     game install and the game happily loads an old one. So the update is a
#     two-phase act with a flag in the config file across the restart: phase one
#     swaps the program directory, phase two offers the mod zip to BeamNG. The
#     flag is written only AFTER the download has validated and staged, so a
#     failed download can never leave a phase-two prompt for an update that was
#     never applied.
#
# Nothing here launches BeamNG.drive. The launch decision is beamtel's, and it
# is deferred until this flow answers -- see the LaunchGate protocol below.

import json
import os
import re
import shutil
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.request
import zipfile

import wx

from bnh_logger import get_logger
from configurator import (
    CONFIG_DIR,
    CONFIG_PATH,
    _read_config_raw,
    _write_config,
    load_config,
)

logger = get_logger()

# Bumped by hand when cutting a tag. release.yml asserts this equals the pushed
# tag with the "v" stripped -- without that check the one hand-maintained
# constant in the update path drifts silently and every client concludes it is
# already up to date.
APP_VERSION = "0.1.4"

REPO = "ironcross32/BeamNG-Accessibility-Mod"
LATEST_URL = "https://api.github.com/repos/%s/releases/latest" % REPO

# Must equal build.py's BUNDLE_NAME. The two files are the same contract seen
# from either end and there is no import that could tie them together.
ASSET_NAME = "BeamNG_accessibility_mod.zip"

# Files the bundle must contain for it to be a bundle at all. Checked before
# anything is staged: a truncated download, an HTML error page saved under a
# .zip name, or a release whose asset is something else entirely all fail here
# rather than halfway through overwriting the user's install.
REQUIRED_MEMBERS = ("beamtel.exe", "bng_screenreader_mod.zip")

UPDATE_DIR = os.path.join(CONFIG_DIR, "update")
STAGED_DIR = os.path.join(UPDATE_DIR, "staged")
HELPER_NAME = "apply_update.cmd"
HELPER_LOG = "apply_update.log"

# GitHub rejects API requests with no User-Agent outright.
USER_AGENT = "BEAM-accessibility-mod-updater/%s" % APP_VERSION

CHECK_TIMEOUT_S = 8
DOWNLOAD_TIMEOUT_S = 30

PENDING_KEY = "pending_update_version"
ENABLED_KEY = "update_check_enabled"


# =========================
#  Launch gate
# =========================


class NullGate:
    """A gate that decides nothing.

    The manual "Check for updates" button runs the same flow as startup, but
    the deferred game launch has long since happened by then -- so its gate must
    be inert rather than merely already-set, or a manual check would fire a
    launch nobody asked for.
    """

    def allow(self):
        pass

    def deny(self):
        pass


# =========================
#  Paths
# =========================


def program_dir():
    """The directory the user actually launched us from.

    Deliberately NOT beamtel's BASE_DIR, which is derived from sys.executable:
    under a Nuitka onefile build that points into the temp extraction directory,
    so an update written there would land in a folder Windows deletes. sys.argv[0]
    is the path the user invoked and is always beside bng_screenreader_mod.zip --
    the same reasoning configurator._get_program_dir() records, and the same
    directory install_mod_interactive() reads the mod zip out of.
    """
    frozen = getattr(sys, "frozen", False) or "__compiled__" in globals()
    if frozen:
        return os.path.dirname(os.path.abspath(sys.argv[0]))
    return os.path.dirname(os.path.abspath(__file__))


# =========================
#  Version comparison
# =========================


_VERSION_RE = re.compile(r"^v?(\d+)(?:\.(\d+))?(?:\.(\d+))?")


def parse_version(text):
    """"v1.2.3" -> (1, 2, 3). None when it cannot be read as a version at all.

    A pre-release suffix ("v1.2.3-beta") keeps its numeric prefix; anything with
    no leading number at all is None. An unreadable REMOTE tag is treated by
    is_newer() as "no update" rather than as newer -- guessing in that direction
    would offer an update on every startup with no way for the user to stop it.
    """
    if not text:
        return None
    m = _VERSION_RE.match(str(text).strip())
    if not m:
        return None
    return tuple(int(g) if g else 0 for g in m.groups())


def is_newer(remote_tag, local=APP_VERSION):
    remote_v = parse_version(remote_tag)
    local_v = parse_version(local)
    if remote_v is None or local_v is None:
        return False
    return remote_v > local_v


# =========================
#  GitHub release query
# =========================


class Release(object):
    def __init__(self, tag, url, size):
        self.tag = tag
        self.url = url
        self.size = size or 0

    @property
    def version(self):
        return self.tag.lstrip("vV")


def check_latest(timeout=CHECK_TIMEOUT_S):
    """The newest published release, or None.

    Every failure -- offline, rate limited, malformed JSON, no matching asset --
    returns None after logging. A check that cannot complete must never block
    startup or the game launch behind it.
    """
    req = urllib.request.Request(
        LATEST_URL,
        method="GET",
        headers={
            "User-Agent": USER_AGENT,
            "Accept": "application/vnd.github+json",
        },
    )
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            data = json.load(resp)
    except urllib.error.HTTPError as e:
        logger.warning("Update check: GitHub returned HTTP %s" % e.code)
        return None
    except urllib.error.URLError as e:
        logger.info("Update check: could not reach GitHub (%s)" % e.reason)
        return None
    except Exception as e:
        logger.warning("Update check failed: %s" % e)
        return None

    if not isinstance(data, dict):
        logger.warning("Update check: unexpected response shape")
        return None

    tag = data.get("tag_name") or ""
    asset_url = None
    asset_size = 0
    for asset in data.get("assets") or []:
        if isinstance(asset, dict) and asset.get("name") == ASSET_NAME:
            asset_url = asset.get("browser_download_url")
            asset_size = asset.get("size") or 0
            break
    if not tag or not asset_url:
        logger.warning(
            "Update check: release %r carries no %s asset" % (tag, ASSET_NAME)
        )
        return None

    return Release(tag, asset_url, asset_size)


# =========================
#  Download / stage
# =========================


def download(url, dest, progress=None, cancelled=None, timeout=DOWNLOAD_TIMEOUT_S):
    """Stream the asset to `dest`, then validate it as one of our bundles.

    Returns None on success, or a message describing what went wrong. The
    validation is the point: everything after this overwrites the user's
    install, so a partial file or an error page saved under a .zip name has to
    be caught here and nowhere later.
    """
    os.makedirs(os.path.dirname(dest), exist_ok=True)
    tmp = dest + ".part"
    req = urllib.request.Request(
        url, method="GET", headers={"User-Agent": USER_AGENT}
    )
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            total = int(resp.headers.get("Content-Length") or 0)
            done = 0
            with open(tmp, "wb") as f:
                while True:
                    if cancelled is not None and cancelled():
                        return "cancelled"
                    chunk = resp.read(64 * 1024)
                    if not chunk:
                        break
                    f.write(chunk)
                    done += len(chunk)
                    if progress is not None:
                        progress(done, total)
    except Exception as e:
        _quiet_remove(tmp)
        return "Download failed: %s" % e

    try:
        with zipfile.ZipFile(tmp) as zf:
            if zf.testzip() is not None:
                raise ValueError("archive contains a corrupt entry")
            names = set(zf.namelist())
            missing = [m for m in REQUIRED_MEMBERS if m not in names]
            if missing:
                raise ValueError(
                    "archive does not contain %s" % ", ".join(missing)
                )
    except Exception as e:
        _quiet_remove(tmp)
        return "The downloaded file is not a valid BEAM update (%s)." % e

    _quiet_remove(dest)
    os.replace(tmp, dest)
    return None


def stage(zip_path, staged_dir=STAGED_DIR):
    """Extract the validated bundle into a scratch directory.

    Returns None on success or a message. The zip-slip guard is here even
    though we build the archive ourselves: this is the one place in the mod
    that writes attacker-influenced paths into the user's program directory,
    and the check costs nothing.
    """
    shutil.rmtree(staged_dir, ignore_errors=True)
    os.makedirs(staged_dir, exist_ok=True)
    try:
        with zipfile.ZipFile(zip_path) as zf:
            for name in zf.namelist():
                norm = name.replace("\\", "/")
                if norm.startswith("/") or os.path.isabs(norm):
                    raise ValueError("absolute path in archive: %s" % name)
                if ".." in norm.split("/"):
                    raise ValueError("parent-directory path in archive: %s" % name)
            zf.extractall(staged_dir)
    except Exception as e:
        shutil.rmtree(staged_dir, ignore_errors=True)
        return "Could not unpack the update: %s" % e

    for member in REQUIRED_MEMBERS:
        if not os.path.isfile(os.path.join(staged_dir, member)):
            shutil.rmtree(staged_dir, ignore_errors=True)
            return "The unpacked update is missing %s." % member
    return None


def _quiet_remove(path):
    try:
        if os.path.exists(path):
            os.remove(path)
    except OSError:
        pass


# =========================
#  Apply (after we exit)
# =========================

# @PLACEHOLDERS@ rather than str.format or %-formatting: this is a batch file,
# where % is the variable syntax and doubling it to survive Python formatting is
# exactly the sort of quiet breakage a helper that runs after we have exited
# gives no chance to see.
_HELPER_TEMPLATE = """@echo off
setlocal
set "PID=@PID@"
set "PROG=@PROG@"
set "STAGED=@STAGED@"
set "LOGFILE=@LOG@"
set "SYS=%SystemRoot%\\System32"

echo [%DATE% %TIME%] waiting for beamtel (started from PID %PID%) to exit> "%LOGFILE%"
set /a TRIES=0
:wait
"%SYS%\\tasklist.exe" /FI "IMAGENAME eq beamtel.exe" /NH 2>nul | "%SYS%\\find.exe" /I "beamtel.exe" >nul
if errorlevel 1 goto gone
set /a TRIES+=1
if %TRIES% GEQ 120 goto timeout
"%SYS%\\ping.exe" -n 2 127.0.0.1 >nul
goto wait

:timeout
echo [%DATE% %TIME%] WARNING: beamtel.exe still running after %TRIES% tries; copying anyway>> "%LOGFILE%"

:gone
rem Give Windows a moment to release the executable image itself.
"%SYS%\\ping.exe" -n 3 127.0.0.1 >nul
echo [%DATE% %TIME%] no beamtel.exe left after %TRIES% tries>> "%LOGFILE%"

rem The Nuitka onefile cache holds the OLD exe's unpacked payload under a fixed
rem name beside the exe (--onefile-tempdir-spec={PROGRAM_DIR}/.appdata with
rem --onefile-cache-mode=cached). It is only safe to remove once the process is
rem gone, which is precisely here. A HALF-removed cache is worse than a stale
rem one -- the new bootstrap can sit on it -- so the outcome is recorded.
if exist "%PROG%\\.appdata" rd /s /q "%PROG%\\.appdata"
if exist "%PROG%\\.appdata" echo [%DATE% %TIME%] WARNING: .appdata survived the delete>> "%LOGFILE%"

echo [%DATE% %TIME%] copying "%STAGED%" -^> "%PROG%">> "%LOGFILE%"
"%SYS%\\robocopy.exe" "%STAGED%" "%PROG%" /E /IS /IT /R:5 /W:2 >> "%LOGFILE%" 2>&1
set RC=%ERRORLEVEL%
echo [%DATE% %TIME%] robocopy exit %RC% >> "%LOGFILE%"
if %RC% GEQ 8 echo [%DATE% %TIME%] copy FAILED; not restarting>> "%LOGFILE%"
if %RC% GEQ 8 goto done

if not exist "%PROG%\\beamtel.exe" echo [%DATE% %TIME%] ERROR: "%PROG%\\beamtel.exe" missing after copy>> "%LOGFILE%"
if not exist "%PROG%\\beamtel.exe" goto done

rem /D sets the new process's working directory: the helper's own is the update
rem folder, and handing that to the program we are restarting is not what it
rem would have had if the user had launched it themselves.
echo [%DATE% %TIME%] starting "%PROG%\\beamtel.exe">> "%LOGFILE%"
start "BEAM" /D "%PROG%" "%PROG%\\beamtel.exe"
echo [%DATE% %TIME%] start returned %ERRORLEVEL% >> "%LOGFILE%"

rem "start" is asynchronous and reports almost nothing, so the launch is
rem VERIFIED rather than assumed. Without this the log's last line is written
rem before the attempt and a silent failure to restart leaves no evidence at
rem all -- which is exactly how this went unexplained once already.
"%SYS%\\ping.exe" -n 6 127.0.0.1 >nul
"%SYS%\\tasklist.exe" /FI "IMAGENAME eq beamtel.exe" /NH 2>nul | "%SYS%\\find.exe" /I "beamtel.exe" >nul
if errorlevel 1 echo [%DATE% %TIME%] ERROR: beamtel.exe did not appear after start>> "%LOGFILE%"
if not errorlevel 1 echo [%DATE% %TIME%] beamtel.exe is running>> "%LOGFILE%"

:done
rd /s /q "%STAGED%"
echo [%DATE% %TIME%] helper finished>> "%LOGFILE%"
(goto) 2>nul & del "%~f0"
"""


def write_helper(staged_dir=STAGED_DIR, prog=None, update_dir=UPDATE_DIR):
    prog = prog or program_dir()
    os.makedirs(update_dir, exist_ok=True)
    path = os.path.join(update_dir, HELPER_NAME)
    body = (
        _HELPER_TEMPLATE.replace("@PID@", str(os.getpid()))
        .replace("@PROG@", os.path.abspath(prog))
        .replace("@STAGED@", os.path.abspath(staged_dir))
        .replace("@LOG@", os.path.join(update_dir, HELPER_LOG))
    )
    with open(path, "w", encoding="ascii", newline="\r\n") as f:
        f.write(body)
    return path


def apply_and_restart(staged_dir=STAGED_DIR):
    """Spawn the detached helper. The caller must then close the app promptly.

    Returns None on success or a message. Detached and window-less: a console
    flashing up behind the closing frame reads as a crash, and a helper in our
    own process group would die with us before it had done anything.
    """
    try:
        helper = write_helper(staged_dir)
    except OSError as e:
        return "Could not write the update helper: %s" % e

    # DETACHED_PROCESS is what keeps the helper alive past our own exit, and it
    # makes CREATE_NO_WINDOW a no-op (the two are documented as mutually
    # exclusive), so the window has to be suppressed the other way -- by giving
    # cmd valid standard handles. beamtel is built --windows-console-mode=disable
    # and therefore HAS no console and no usable std handles; a detached cmd that
    # inherits those finds neither a console nor anywhere to write and allocates
    # a console of its own, which is the stray window left behind by an update.
    DETACHED_PROCESS = 0x00000008
    CREATE_NEW_PROCESS_GROUP = 0x00000200
    try:
        subprocess.Popen(
            ["cmd", "/c", helper],
            cwd=UPDATE_DIR,
            close_fds=True,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            creationflags=(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP),
        )
    except Exception as e:
        return "Could not start the update helper: %s" % e
    return None


# =========================
#  Pending-update flag
# =========================


def _patch_config(key, value):
    """Set one key in the config file, preserving everything else verbatim.

    Read-modify-write of the RAW file rather than load_config()/save: that path
    merges defaults, coerces and re-writes the whole document, which is right
    for the settings UI and wrong for a single flag written from a background
    flow that has no business rewriting the user's settings.
    """
    raw, os_error = _read_config_raw(CONFIG_PATH)
    if os_error is not None or not isinstance(raw, dict):
        raw = load_config()
    raw[key] = value
    _write_config(CONFIG_PATH, raw)


def set_pending_update(tag):
    _patch_config(PENDING_KEY, tag)


def clear_pending_update():
    _patch_config(PENDING_KEY, "")


# =========================
#  wx progress plumbing
# =========================


def _run_with_progress(parent, title, message, work, cancellable=False):
    """Run `work(report, cancelled)` off the GUI thread behind a progress dialog.

    `work` returns (result, error_message). Returns the same pair, with
    error_message set to the literal "cancelled" if the user cancelled.
    """
    state = {"done": 0, "total": 0, "cancel": False, "result": None, "error": None}

    def report(done, total):
        state["done"] = done
        state["total"] = total

    def cancelled():
        return state["cancel"]

    def runner():
        try:
            state["result"], state["error"] = work(report, cancelled)
        except Exception as e:  # a crash here must not hang the dialog
            state["error"] = str(e)

    style = wx.PD_APP_MODAL | wx.PD_AUTO_HIDE
    if cancellable:
        style |= wx.PD_CAN_ABORT
    dlg = wx.ProgressDialog(title, message, maximum=100, parent=parent, style=style)
    thread = threading.Thread(target=runner, name="beamtel-updater", daemon=True)
    thread.start()
    try:
        while thread.is_alive():
            total = state["total"]
            if total > 0:
                pct = min(100, int(state["done"] * 100 / total))
                keep = dlg.Update(
                    pct, "%s\n%.1f of %.1f MB" % (
                        message,
                        state["done"] / 1048576.0,
                        total / 1048576.0,
                    )
                )[0]
            else:
                keep = dlg.Pulse(message)[0]
            if cancellable and not keep:
                state["cancel"] = True
            wx.Yield()
            time.sleep(0.05)
        thread.join(timeout=2.0)
    finally:
        dlg.Destroy()

    if state["cancel"] and not state["error"]:
        state["error"] = "cancelled"
    return state["result"], state["error"]


# =========================
#  The flow
# =========================


def run_startup_flow(frame, gate, manual=False):
    """Decide about updates, then release the deferred game launch.

    Runs on the GUI thread. Exactly one of gate.allow() / gate.deny() fires on
    every path -- allow() is in a finally, so a crash in here cannot leave the
    engine thread waiting out its whole timeout before starting the game.
    """
    decided = {"value": False}

    def allow():
        if not decided["value"]:
            decided["value"] = True
            gate.allow()

    def deny():
        decided["value"] = True
        gate.deny()

    try:
        cfg = load_config()
        pending = str(cfg.get(PENDING_KEY) or "").strip()
        if pending and not manual:
            _phase_two(frame, pending)
            return
        _check_and_offer(frame, cfg, manual, allow, deny)
    except Exception as e:
        logger.error("Updater flow failed: %s" % e)
    finally:
        allow()


def _phase_two(frame, pending):
    """We have just restarted into a freshly copied build. Offer the mod zip.

    The flag is cleared whichever way the user answers: it records "an update
    was applied and the mod has not been offered for it yet", and it has been
    offered now. Leaving it set on a decline would re-ask on every start.
    """
    logger.info("Pending update %s: offering the mod install" % pending)
    from config_ui import install_mod_interactive

    answer = wx.MessageBox(
        "BEAM has been updated to version %s.\n\n"
        "Install the updated BeamNG.drive mod now?" % pending,
        "Install the updated mod",
        wx.YES_NO | wx.YES_DEFAULT | wx.ICON_QUESTION,
        frame,
    )
    installed = False
    try:
        if answer == wx.YES:
            installed = bool(install_mod_interactive(frame))
        else:
            logger.info("User declined the mod install for update %s" % pending)
    finally:
        try:
            clear_pending_update()
        except Exception as e:
            logger.error("Could not clear the pending update flag: %s" % e)
    logger.info(
        "Pending update %s resolved (mod installed: %s)" % (pending, installed)
    )


def _check_and_offer(frame, cfg, manual, allow, deny):
    if not manual and not cfg.get(ENABLED_KEY, True):
        return

    release, error = _run_with_progress(
        frame,
        "Checking for updates",
        "Contacting GitHub...",
        lambda report, cancelled: (check_latest(), None),
    )
    if release is None or error:
        logger.info("Update check found nothing to install")
        if manual:
            wx.MessageBox(
                "Could not check for updates right now.",
                "Update Check Failed",
                wx.OK | wx.ICON_WARNING,
                frame,
            )
        return

    if not is_newer(release.tag):
        logger.info(
            "Up to date: running %s, latest release is %s"
            % (APP_VERSION, release.tag)
        )
        if manual:
            wx.MessageBox(
                "You are running the latest version (%s)." % APP_VERSION,
                "No Update Available",
                wx.OK | wx.ICON_INFORMATION,
                frame,
            )
        return

    # The offer is the question and nothing else. The release notes are commit
    # messages rather than a maintained changelog, so they explain little at
    # some length; the restart and the mod prompt that follows it both announce
    # themselves when they happen, and a link is painful to get out of a message
    # box with a screen reader. Being accessible is not the same as being chatty.
    answer = wx.MessageBox(
        "You have version %s. Version %s is available.\n\n"
        "Install it now?" % (APP_VERSION, release.version),
        "Update Available",
        wx.YES_NO | wx.YES_DEFAULT | wx.ICON_QUESTION,
        frame,
    )
    if answer != wx.YES:
        logger.info("User declined update %s" % release.tag)
        return

    os.makedirs(UPDATE_DIR, exist_ok=True)
    zip_path = os.path.join(UPDATE_DIR, ASSET_NAME)

    _, error = _run_with_progress(
        frame,
        "Downloading update",
        "Downloading %s..." % ASSET_NAME,
        lambda report, cancelled: (
            None,
            download(release.url, zip_path, progress=report, cancelled=cancelled),
        ),
        cancellable=True,
    )
    if error == "cancelled":
        logger.info("User cancelled the download of %s" % release.tag)
        _quiet_remove(zip_path)
        allow()
        return
    if error:
        _fail(frame, error, allow)
        return

    error = stage(zip_path)
    if error:
        _fail(frame, error, allow)
        return

    # Only now, with a validated and unpacked update on disk, does the pending
    # flag go in: it is a promise to the next run that there is something to
    # install, and a promise made before staging could not be kept.
    try:
        set_pending_update(release.version)
    except Exception as e:
        _fail(frame, "Could not record the pending update: %s" % e, allow)
        return

    # Only at the point of no return does the launch get denied. Denying it
    # earlier -- at the "yes, update" answer -- would strand the user with no
    # game if the download then failed or they cancelled it, because the engine
    # thread reads the decision the moment the gate opens and never looks again.
    deny()
    error = apply_and_restart()
    if error:
        try:
            clear_pending_update()
        except Exception:
            pass
        _fail(frame, error, allow)
        return

    logger.info("Update %s staged; restarting to apply it" % release.tag)
    wx.MessageBox(
        "BEAM will now close and restart on the new version.",
        "Restarting to Update",
        wx.OK | wx.ICON_INFORMATION,
        frame,
    )
    frame.Close(True)


def _fail(frame, message, allow):
    """Report a failed update and fall back to a completely normal startup."""
    logger.error("Update failed: %s" % message)
    wx.MessageBox(
        "The update could not be installed.\n\n%s\n\n"
        "BEAM will carry on running the version you already have." % message,
        "Update Failed",
        wx.OK | wx.ICON_ERROR,
        frame,
    )
    allow()
