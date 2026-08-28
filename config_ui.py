# config_ui.py
# UI components for BeamTel configuration.
# Extracted from configurator.py so the panel can be embedded in the unified app.

import datetime
import json
import os
import shutil
import tempfile
import zipfile

import wx

import secretstore
from configurator import (
    load_config,
    _write_config,
    CONFIG_PATH,
    DEFAULT_CONFIG,
    list_speech_backends,
    list_speech_voices,
    test_speech_voice,
    play_test_tone,
    AUDIO_TEST_OK,
    list_wasapi_output_devices,
    _get_program_dir,
)

_AUTO_SAVE_DELAY_MS = 2000

_MOD_ZIP = "bng_screenreader_mod.zip"
# Written into the mod zip by build.py's package_mod(). See MOD_VERSION_MEMBER
# there for why a file mtime cannot answer this question.
_MOD_VERSION_MEMBER = "bnvda_mod_version.json"
_BNVDA_APP_NAME = "bnvdaHook"


def _freeroam_layout_path(local_appdata):
    return os.path.join(
        local_appdata,
        "BeamNG",
        "BeamNG.drive",
        "current",
        "settings",
        "ui_apps",
        "layouts",
        "default",
        "freeroam.uilayout.json",
    )


def _load_bnvda_layout(layout_path):
    """Load a freeroam layout and count obsolete BNVDA app entries."""
    with open(layout_path, "r", encoding="utf-8-sig") as layout_file:
        layout = json.load(layout_file)

    if not isinstance(layout, dict):
        raise ValueError("The layout's top-level JSON value is not an object.")
    apps = layout.get("apps")
    if not isinstance(apps, list):
        raise ValueError("The layout does not contain an 'apps' list.")

    entry_count = sum(
        1
        for app in apps
        if isinstance(app, dict) and app.get("appName") == _BNVDA_APP_NAME
    )
    return layout, entry_count


def _remove_bnvda_layout_entries(layout):
    """Remove every obsolete BNVDA app entry from a validated layout."""
    layout["apps"] = [
        app
        for app in layout["apps"]
        if not (isinstance(app, dict) and app.get("appName") == _BNVDA_APP_NAME)
    ]


def _write_layout_with_backup(layout_path, layout):
    """Back up and atomically replace a validated BeamNG UI layout."""
    layout_dir = os.path.dirname(layout_path)
    layout_name = os.path.basename(layout_path)
    timestamp = datetime.datetime.now().strftime("%Y%m%d-%H%M%S-%f")
    backup_path = f"{layout_path}.pre-bnvda-removal-{timestamp}.bak"
    temp_path = None

    try:
        with tempfile.NamedTemporaryFile(
            mode="w",
            encoding="utf-8",
            newline="\n",
            prefix=f"{layout_name}.",
            suffix=".tmp",
            dir=layout_dir,
            delete=False,
        ) as temp_file:
            temp_path = temp_file.name
            json.dump(layout, temp_file, ensure_ascii=False, indent=2)
            temp_file.write("\n")
            temp_file.flush()
            os.fsync(temp_file.fileno())

        shutil.copy2(layout_path, backup_path)
        os.replace(temp_path, layout_path)
        temp_path = None
        return backup_path
    finally:
        if temp_path and os.path.exists(temp_path):
            try:
                os.remove(temp_path)
            except OSError:
                pass



def _mod_zip_version(path):
    """(parsed, raw) for a mod zip's stamped version, or None when unstamped.

    Unstamped is a real answer, not an error: it means a zip built before
    build.py started stamping, which is by definition older than any stamped
    one. A zip we cannot open at all is also None -- the caller then falls back
    to a byte comparison, and if that fails too it asks.
    """
    try:
        with zipfile.ZipFile(path) as zf:
            raw = json.loads(zf.read(_MOD_VERSION_MEMBER).decode("utf-8"))
    except Exception:
        return None
    version = raw.get("version") if isinstance(raw, dict) else None
    if not isinstance(version, str):
        return None
    from updater import parse_version

    parsed = parse_version(version)
    if parsed is None:
        return None
    return parsed, version


def _same_bytes(a, b):
    """True when two files are byte-identical. Unreadable counts as 'not sure'."""
    try:
        if os.path.getsize(a) != os.path.getsize(b):
            return False
        with open(a, "rb") as fa, open(b, "rb") as fb:
            while True:
                ca, cb = fa.read(65536), fb.read(65536)
                if ca != cb:
                    return False
                if not ca:
                    return True
    except OSError:
        return False


def install_mod_interactive(parent):
    """Run the mod installation flow, showing wx dialogs as needed. Call from any wx context.

    Returns True only when it reaches the "Installation Complete" box. The
    button caller ignores that, but the updater's phase-two prompt needs to know
    which of the early returns it took -- every one of them has already told the
    user what went wrong, so the value is for the log, not for a second dialog.
    """
    # Step 1 (registry check removed): installation is confirmed by the mods
    # directory existing in step 3.

    # Step 2: Locate the zip alongside this program.
    src_zip = os.path.join(_get_program_dir(), _MOD_ZIP)
    if not os.path.isfile(src_zip):
        wx.MessageBox(
            f"'{_MOD_ZIP}' was not found in the program directory:\n{os.path.dirname(src_zip)}\n\n"
            "Installation cannot proceed.",
            "Mod File Not Found",
            wx.OK | wx.ICON_ERROR,
            parent,
        )
        return False

    # Step 3: Confirm the game's mods directory exists.
    local_appdata = os.getenv("LOCALAPPDATA", os.path.expanduser("~"))
    mods_dir = os.path.join(local_appdata, "BeamNG", "BeamNG.drive", "current", "mods")
    if not os.path.isdir(mods_dir):
        wx.MessageBox(
            "The BeamNG.drive mods directory was not found:\n" + mods_dir + "\n\n"
            "Please run BeamNG.drive at least once before installing the mod.",
            "Mods Directory Not Found",
            wx.OK | wx.ICON_ERROR,
            parent,
        )
        return False

    # Step 4: Decide whether to copy.
    #
    # By the STAMPED version, never by file mtime. The two zips routinely live
    # on different machines and are moved by tools that each treat mtime
    # differently (shutil.copy2 preserves it, zipfile.extractall invents it,
    # robocopy forwards it), so an mtime ordering is a comparison of clocks --
    # which is what made a freshly downloaded release announce itself as older
    # than the copy it had just replaced.
    dst_zip = os.path.join(mods_dir, _MOD_ZIP)
    do_copy = False
    mod_result = "kept the existing installed file"
    if not os.path.isfile(dst_zip):
        do_copy = True
    elif _same_bytes(src_zip, dst_zip):
        mod_result = "the installed file is already identical"
    else:
        src_ver = _mod_zip_version(src_zip)
        dst_ver = _mod_zip_version(dst_zip)
        if src_ver and (dst_ver is None or src_ver[0] >= dst_ver[0]):
            # Newer, or the same version rebuilt, or replacing a zip from before
            # stamping existed. All three are the ordinary update.
            do_copy = True
        elif src_ver and dst_ver:
            ans = wx.MessageBox(
                "The mod in the program directory is version %s, but version %s "
                "is already installed.\n"
                "Do you want to install the older one anyway?"
                % (src_ver[1], dst_ver[1]),
                "Older Version Detected",
                wx.YES_NO | wx.NO_DEFAULT | wx.ICON_WARNING,
                parent,
            )
            if ans == wx.YES:
                do_copy = True
        else:
            # Neither zip carries a version, so there is nothing to order them
            # by. Say exactly that rather than inventing a direction.
            ans = wx.MessageBox(
                "The mod in the program directory differs from the one already "
                "installed, but neither carries a version stamp, so which is "
                "newer cannot be determined.\n\n"
                "Install the one from the program directory?",
                "Cannot Determine Version",
                wx.YES_NO | wx.YES_DEFAULT | wx.ICON_QUESTION,
                parent,
            )
            if ans == wx.YES:
                do_copy = True

    # Perform the copy.
    if do_copy:
        try:
            shutil.copy2(src_zip, dst_zip)
        except Exception as e:
            wx.MessageBox(
                f"Failed to copy mod file:\n{e}",
                "Installation Failed",
                wx.OK | wx.ICON_ERROR,
                parent,
            )
            return False

        # Step 5: Verify the result.
        if not os.path.isfile(dst_zip):
            wx.MessageBox(
                "The copy appeared to succeed but the destination file cannot be found.\n"
                "Installation may have failed.",
                "Verification Failed",
                wx.OK | wx.ICON_WARNING,
                parent,
            )
            return False
        mod_result = "copied the mod file"

    # Step 6: Offer to remove the obsolete HUD app from the freeroam layout.
    layout_path = _freeroam_layout_path(local_appdata)
    layout_result = "layout file was not found; no cleanup was needed"
    backup_path = None
    if os.path.isfile(layout_path):
        try:
            layout, entry_count = _load_bnvda_layout(layout_path)
        except (OSError, UnicodeError, json.JSONDecodeError, ValueError) as e:
            layout_result = "could not inspect the layout; it was not changed"
            wx.MessageBox(
                f"The mod file was {mod_result}, but the BeamNG.drive freeroam UI "
                f"layout could not be read:\n{layout_path}\n\n{e}\n\n"
                "The layout was not changed.",
                "Invalid Freeroam Layout",
                wx.OK | wx.ICON_WARNING,
                parent,
            )
        else:
            if entry_count == 0:
                layout_result = "obsolete BNVDA Hook entry was not present"
            else:
                noun = "entry" if entry_count == 1 else "entries"
                answer = wx.MessageBox(
                    f"The freeroam UI layout contains {entry_count} obsolete BNVDA "
                    f"Hook {noun}. The mod now starts automatically when the game "
                    "starts, so these HUD entries are no longer needed.\n\n"
                    "Do you want to back up the layout and remove them?",
                    "Remove Obsolete HUD App",
                    wx.YES_NO | wx.NO_DEFAULT | wx.ICON_QUESTION,
                    parent,
                )
                if answer == wx.YES:
                    _remove_bnvda_layout_entries(layout)
                    try:
                        backup_path = _write_layout_with_backup(layout_path, layout)
                    except Exception as e:
                        layout_result = "cleanup failed; the original layout was preserved"
                        wx.MessageBox(
                            f"The mod file was {mod_result}, but the obsolete BNVDA "
                            "Hook layout entry could not be removed:\n"
                            f"{layout_path}\n\n{e}\n\n"
                            "The original layout was preserved.",
                            "Layout Cleanup Failed",
                            wx.OK | wx.ICON_WARNING,
                            parent,
                        )
                    else:
                        layout_result = f"removed {entry_count} obsolete {noun}"
                else:
                    layout_result = f"kept {entry_count} obsolete {noun} at your request"

    backup_detail = f"\nLayout backup: {backup_path}" if backup_path else ""

    wx.MessageBox(
        "The mod installation is complete.\n\n"
        f"Mod file: {mod_result}.\n"
        f"Freeroam layout: {layout_result}.\n\n"
        f"{dst_zip}{backup_detail}",
        "Installation Complete",
        wx.OK | wx.ICON_INFORMATION,
        parent,
    )
    return True


class LabelAccessible(wx.Accessible):
    """MSAA accessible object that returns a fixed name for screen readers."""

    def __init__(self, win, name):
        super().__init__(win)
        self._name = name

    def GetName(self, childId):
        return (wx.ACC_OK, self._name)


def _label_spin(ctrl, name):
    """Set accessible Name on a SpinCtrl or SpinCtrlDouble via wx.Accessible.

    Both are native Win32 composites (an Edit plus an UpDown).  Focus lands on
    the inner Edit, which does not inherit the wrapper's SetName, so the name
    has to be applied to both halves or the screen reader announces nothing.
    """
    ctrl.SetAccessible(LabelAccessible(ctrl, name))
    for child in ctrl.GetChildren():
        if isinstance(child, wx.TextCtrl):
            child.SetAccessible(LabelAccessible(child, name))
            return


def _group(parent, label):
    """Create a StaticBox and its sizer; returns (box, sizer).

    Children MUST be created with the returned box as their parent, not with
    `parent` -- on Windows that parenting is what makes MSAA/UIA nest the
    controls inside the group, and nesting is how a screen reader knows to
    announce the group name when focus enters it.  Parenting them to the panel
    instead leaves the box a mere sibling: visible, but silent.
    """
    box = wx.StaticBox(parent, label=label)
    return box, wx.StaticBoxSizer(box, wx.VERTICAL)


def _owns_focus(ctrl):
    """True if `ctrl` holds keyboard focus, or one of its inner windows does.

    Descendants matter: SpinCtrl and SpinCtrlDouble are native composites, so
    focus actually sits on their inner TextCtrl and never on the wrapper an
    identity test would compare against.
    """
    win = wx.Window.FindFocus()
    while win is not None:
        if win is ctrl:
            return True
        win = win.GetParent()
    return False


def _enable(ctrl, enabled, focus_fallback=None):
    """Enable or disable `ctrl` without ever stranding keyboard focus on it.

    Disabling the focused window on Windows drops focus to the parent, or to
    nothing at all, and a screen reader simply loses its place.  Move focus
    somewhere deliberate first -- normally the checkbox that governs the
    control being disabled.
    """
    if not enabled and focus_fallback is not None and _owns_focus(ctrl):
        focus_fallback.SetFocus()
    ctrl.Enable(enabled)


def _set_row(sizer, ctrls, enabled, focus_fallback):
    """Show/hide a dependent row and enable/disable the controls in it.

    Focus is rescued *before* anything else happens.  Hiding a window drops
    focus just as disabling one does, so guarding only the Enable call would be
    too late -- ShowItems would already have dumped focus on the parent, and by
    then the control no longer owns it for the guard to notice.
    """
    if not enabled and any(_owns_focus(c) for c in ctrls):
        focus_fallback.SetFocus()
    sizer.ShowItems(enabled)
    for c in ctrls:
        c.Enable(enabled)


def _focusable_leaves(win):
    """Recursively collect leaf focusable controls in child (tab) order."""
    result = []
    for child in win.GetChildren():
        sub = _focusable_leaves(child)
        if sub:
            result.extend(sub)
        elif child.AcceptsFocusFromKeyboard() and child.IsEnabled() and child.IsShown():
            result.append(child)
    return result


def wrap_nav_key(evt, page):
    """EVT_NAVIGATION_KEY handler for notebook pages.

    At the forward boundary (Tab from last control) and backward boundary
    (Shift+Tab from first control), focus the Notebook tab bar so it remains
    reachable in the keyboard cycle.  The complementary Notebook-level handler
    in BeamTelFrame then routes Tab/Shift+Tab from the tab bar into the page.

    Bind this on the page window itself (not on individual controls).
    """
    if evt.IsWindowChange():
        evt.Skip()
        return
    leaves = _focusable_leaves(page)
    if not leaves:
        evt.Skip()
        return
    focused = wx.Window.FindFocus()
    at_boundary = (evt.GetDirection() and focused is leaves[-1]) or (
        not evt.GetDirection() and focused is leaves[0]
    )
    if at_boundary:
        notebook = page.GetParent()
        if isinstance(notebook, wx.Notebook):
            notebook.SetFocus()
        else:
            # No notebook parent; fall back to direct wrap.
            if evt.GetDirection():
                leaves[0].SetFocus()
            else:
                leaves[-1].SetFocus()
    else:
        evt.Skip()


def _bind_spin_double_page_keys(ctrl, on_change, page_multiplier=10):
    """Add PageUp/Down large-step support to a SpinCtrlDouble.

    wx.SpinCtrl (integer) has native Windows page-key support; SpinCtrlDouble
    does not — its inner TextCtrl receives keyboard focus but has no page-step
    logic.  This binds EVT_KEY_DOWN on that inner TextCtrl to fill the gap.
    on_change() is called after each programmatic value change so that the
    auto-save timer is notified.
    """
    inc = ctrl.GetIncrement()
    page_inc = inc * page_multiplier

    def on_key(evt):
        kc = evt.GetKeyCode()
        if kc == wx.WXK_PAGEUP:
            ctrl.SetValue(min(ctrl.GetMax(), ctrl.GetValue() + page_inc))
            on_change()
        elif kc == wx.WXK_PAGEDOWN:
            ctrl.SetValue(max(ctrl.GetMin(), ctrl.GetValue() - page_inc))
            on_change()
        else:
            evt.Skip()

    for child in ctrl.GetChildren():
        if isinstance(child, wx.TextCtrl):
            child.Bind(wx.EVT_KEY_DOWN, on_key)
            return
    ctrl.Bind(wx.EVT_KEY_DOWN, on_key)  # fallback


class ConfigPanel(wx.ScrolledWindow):
    """Configuration panel that can be embedded in a Notebook tab or standalone frame."""

    def __init__(self, parent):
        super().__init__(parent, style=wx.TAB_TRAVERSAL)
        self.SetScrollRate(0, 10)
        self._loading = False

        # Auto-save timer: fires _auto_save 2s after the last UI change.
        self._save_timer = wx.Timer(self)
        self.Bind(wx.EVT_TIMER, self._auto_save, self._save_timer)

        # Tab wrapping within this page.
        self.Bind(wx.EVT_NAVIGATION_KEY, lambda evt: wrap_nav_key(evt, self))

        vbox = wx.BoxSizer(wx.VERTICAL)

        # --- Speech group ---
        sb_speech, speech = _group(self, "Speech")
        grid = wx.FlexGridSizer(0, 2, 6, 8)
        grid.AddGrowableCol(1, 1)
        lbl_backend = wx.StaticText(sb_speech, label="Speech Backend:")
        self.choice_backend = wx.Choice(sb_speech)
        self.choice_backend.SetToolTip(
            "Which screen reader or speech engine to talk to. Auto picks the "
            "best one that is running."
        )
        self.choice_backend.SetName("Speech Backend")
        grid.Add(lbl_backend, 0, wx.ALIGN_CENTER_VERTICAL)
        grid.Add(self.choice_backend, 1, wx.EXPAND)
        lbl_voice = wx.StaticText(sb_speech, label="Voice:")
        self.choice_voice = wx.Choice(sb_speech)
        self.choice_voice.SetToolTip(
            "Voice to use, where the backend allows choosing one. Screen "
            "readers use their own voice settings."
        )
        self.choice_voice.SetName("Voice")
        grid.Add(lbl_voice, 0, wx.ALIGN_CENTER_VERTICAL)
        grid.Add(self.choice_voice, 1, wx.EXPAND)
        lbl_rate = wx.StaticText(sb_speech, label="Rate (0 to 100):")
        self.spin_rate = wx.SpinCtrl(sb_speech, min=0, max=100)
        self.spin_rate.SetToolTip("Speech rate: 0 (slow) to 100 (fast).")
        self.spin_rate.SetName("Speech Rate")
        _label_spin(self.spin_rate, "Speech Rate")
        grid.Add(lbl_rate, 0, wx.ALIGN_CENTER_VERTICAL)
        grid.Add(self.spin_rate, 1, wx.EXPAND)
        lbl_vol = wx.StaticText(sb_speech, label="Volume (0 to 100):")
        self.spin_volume = wx.SpinCtrl(sb_speech, min=0, max=100)
        self.spin_volume.SetToolTip("Speech volume: 0 to 100.")
        self.spin_volume.SetName("Speech Volume")
        _label_spin(self.spin_volume, "Speech Volume")
        grid.Add(lbl_vol, 0, wx.ALIGN_CENTER_VERTICAL)
        grid.Add(self.spin_volume, 1, wx.EXPAND)
        speech.Add(grid, 0, wx.LEFT | wx.RIGHT | wx.TOP | wx.EXPAND, 6)
        # Says out loud what the selected backend can and cannot do, so the
        # greyed-out controls below are explained rather than just inert.
        self.lbl_speech_caps = wx.StaticText(sb_speech, label="")
        self.lbl_speech_caps.SetName("Backend Capabilities")
        speech.Add(self.lbl_speech_caps, 0, wx.LEFT | wx.RIGHT | wx.TOP, 6)
        speech_btn_sizer = wx.BoxSizer(wx.HORIZONTAL)
        btn_refresh = wx.Button(sb_speech, label="Refresh Voices")
        btn_refresh.SetToolTip("Re-detect speech backends and voices.")
        btn_test = wx.Button(sb_speech, label="Test Voice")
        btn_test.SetToolTip(
            "Speak a short test line with the selected voice, rate, and volume."
        )
        speech_btn_sizer.AddStretchSpacer()
        speech_btn_sizer.Add(btn_refresh, 0, wx.RIGHT, 5)
        speech_btn_sizer.Add(btn_test)
        speech.Add(speech_btn_sizer, 0, wx.ALL | wx.EXPAND, 6)
        vbox.Add(speech, 0, wx.ALL | wx.EXPAND, 10)

        # --- General group ---
        sb_gen, gen = _group(self, "General")
        self.rb_units = wx.RadioBox(
            sb_gen,
            label="Display Units",
            choices=["Imperial (mph, \u00b0F, psi)", "Metric (km/h, \u00b0C, bar)"],
            majorDimension=1,
            style=wx.RA_SPECIFY_ROWS,
        )
        self.rb_units.SetToolTip("Choose how speeds, temps, and pressures are spoken.")
        gen.Add(self.rb_units, 0, wx.ALL | wx.EXPAND, 6)
        self.rb_proto = wx.RadioBox(
            sb_gen,
            label="Telemetry Protocol",
            choices=["Extended", "OutGauge"],
            majorDimension=1,
            style=wx.RA_SPECIFY_ROWS,
        )
        self.rb_proto.SetToolTip(
            "Must match the protocol selected in the BeamNG.drive UI app."
        )
        gen.Add(self.rb_proto, 0, wx.LEFT | wx.RIGHT | wx.BOTTOM | wx.EXPAND, 6)
        self.chk_launch_beamng = wx.CheckBox(
            sb_gen, label="Launch BeamNG.drive on startup"
        )
        self.chk_launch_beamng.SetToolTip(
            "Automatically launch BeamNG.drive via Steam when BeamTel starts."
        )
        gen.Add(self.chk_launch_beamng, 0, wx.LEFT | wx.RIGHT | wx.BOTTOM, 6)

        # Renderer row — visible only when launch checkbox is checked.
        self._renderer_row = wx.BoxSizer(wx.HORIZONTAL)
        lbl_renderer = wx.StaticText(sb_gen, label="Graphics API:")
        self.choice_renderer = wx.Choice(sb_gen, choices=["Direct3D 11", "Vulkan"])
        self.choice_renderer.SetName("Graphics API")
        self.choice_renderer.SetToolTip(
            "Which graphics API BeamNG.drive should use when launched automatically."
        )
        self._renderer_row.Add(lbl_renderer, 0, wx.ALIGN_CENTER_VERTICAL | wx.RIGHT, 6)
        self._renderer_row.Add(self.choice_renderer, 1, wx.EXPAND)
        gen.Add(self._renderer_row, 0, wx.EXPAND | wx.LEFT | wx.RIGHT | wx.BOTTOM, 6)

        self.chk_update_check = wx.CheckBox(
            sb_gen, label="Check for updates on startup"
        )
        self.chk_update_check.SetToolTip(
            "Ask GitHub for the newest release when BeamTel starts, and offer to "
            "install it. Nothing is downloaded without your answer."
        )
        gen.Add(self.chk_update_check, 0, wx.LEFT | wx.RIGHT | wx.BOTTOM, 6)

        vbox.Add(gen, 0, wx.LEFT | wx.RIGHT | wx.BOTTOM | wx.EXPAND, 10)

        # --- Automatic Announcements Group ---
        sb_auto, auto_sizer = _group(self, "Automatic announcements")

        self.chk_announce_turn_signals = wx.CheckBox(sb_auto, label="Announce turn signals")
        self.chk_announce_turn_signals.SetToolTip(
            "Speak when the left, right, or hazard turn signals are activated or deactivated."
        )
        auto_sizer.Add(self.chk_announce_turn_signals, 0, wx.ALL, 6)

        self.chk_announce_speed = wx.CheckBox(sb_auto, label="Speed announcements")
        self.chk_announce_speed.SetToolTip(
            "Automatically speak the current speed each time it crosses an interval threshold."
        )
        auto_sizer.Add(self.chk_announce_speed, 0, wx.LEFT | wx.RIGHT, 6)

        self._speed_interval_sizer = wx.BoxSizer(wx.HORIZONTAL)
        lbl_interval = wx.StaticText(sb_auto, label="Announce every:")
        self.choice_speed_interval = wx.Choice(sb_auto)
        self.choice_speed_interval.SetName("Speed announcement interval")
        self.choice_speed_interval.SetToolTip(
            "How many speed units must be crossed before a speed announcement is made."
        )
        self._speed_interval_sizer.Add(lbl_interval, 0, wx.ALIGN_CENTER_VERTICAL | wx.RIGHT, 6)
        self._speed_interval_sizer.Add(self.choice_speed_interval, 1, wx.EXPAND)
        auto_sizer.Add(self._speed_interval_sizer, 0, wx.ALL | wx.EXPAND, 6)

        self.chk_announce_gear = wx.CheckBox(sb_auto, label="Gear change announcements")
        self.chk_announce_gear.SetToolTip(
            "Speak the new gear each time the vehicle changes gears."
        )
        auto_sizer.Add(self.chk_announce_gear, 0, wx.LEFT | wx.RIGHT | wx.BOTTOM, 6)

        self.chk_scanner_callout = wx.CheckBox(sb_auto, label="Periodic scanner distance callouts")
        self.chk_scanner_callout.SetToolTip(
            "When the vehicle scanner is active and has a target, periodically announce "
            "the distance and direction to that target."
        )
        auto_sizer.Add(self.chk_scanner_callout, 0, wx.LEFT | wx.RIGHT, 6)

        self._callout_interval_sizer = wx.BoxSizer(wx.HORIZONTAL)
        lbl_callout = wx.StaticText(sb_auto, label="Callout every:")
        self.choice_callout_interval = wx.Choice(sb_auto)
        self.choice_callout_interval.SetName("Scanner callout interval")
        self.choice_callout_interval.SetToolTip(
            "How often (in seconds) to announce the scanner target distance and direction."
        )
        self._callout_interval_sizer.Add(lbl_callout, 0, wx.ALIGN_CENTER_VERTICAL | wx.RIGHT, 6)
        self._callout_interval_sizer.Add(self.choice_callout_interval, 1, wx.EXPAND)
        auto_sizer.Add(self._callout_interval_sizer, 0, wx.ALL | wx.EXPAND, 6)

        self.chk_ui_nav_hold = wx.CheckBox(
            sb_auto, label="Quiet menu navigation while holding a direction"
        )
        self.chk_ui_nav_hold.SetToolTip(
            "When holding a direction to move quickly through a menu, list, or slider, "
            "speak the first item and then stay silent until you stop, announcing "
            "where you landed."
        )
        auto_sizer.Add(self.chk_ui_nav_hold, 0, wx.LEFT | wx.RIGHT | wx.BOTTOM, 6)

        vbox.Add(auto_sizer, 0, wx.LEFT | wx.RIGHT | wx.BOTTOM | wx.EXPAND, 10)

        # --- Compass Clicks Group ---
        sb_compass, compass_sizer = _group(self, "Compass Clicks")
        compass_grid = wx.FlexGridSizer(0, 2, 6, 8)
        compass_grid.AddGrowableCol(1, 1)

        lbl_compass_interval = wx.StaticText(sb_compass, label="Click Interval (degrees):")
        self.spin_compass_interval = wx.SpinCtrl(sb_compass, min=1, max=90)
        self.spin_compass_interval.SetToolTip(
            "The number of degrees of rotation before a compass click is heard (1-90)."
        )
        self.spin_compass_interval.SetName("Compass Click Interval")
        _label_spin(self.spin_compass_interval, "Compass Click Interval")
        compass_grid.Add(lbl_compass_interval, 0, wx.ALIGN_CENTER_VERTICAL)
        compass_grid.Add(self.spin_compass_interval, 0, wx.EXPAND)

        self.chk_compass_highlight = wx.CheckBox(sb_compass, label="Highlight Every:")
        self.chk_compass_highlight.SetToolTip(
            "Play a distinct sound on a certain click."
        )

        self.spin_compass_highlight_nth = wx.SpinCtrl(sb_compass, min=2, max=100)
        self.spin_compass_highlight_nth.SetToolTip(
            "Play the highlight sound on every Nth click (e.g., 4 for quadrants)."
        )
        # The unit lives in a separate StaticText that focus never lands on, so
        # fold it into the name -- otherwise the value is announced bare.
        self.spin_compass_highlight_nth.SetName("Highlight every N clicks")
        _label_spin(self.spin_compass_highlight_nth, "Highlight every N clicks")

        compass_grid.Add(self.chk_compass_highlight, 0, wx.ALIGN_CENTER_VERTICAL)

        self._highlight_row = wx.BoxSizer(wx.HORIZONTAL)
        self._highlight_row.Add(self.spin_compass_highlight_nth, 1, wx.EXPAND)
        self._highlight_row.Add(
            wx.StaticText(sb_compass, label=" clicks"),
            0,
            wx.ALIGN_CENTER_VERTICAL | wx.LEFT,
            4,
        )
        compass_grid.Add(self._highlight_row, 1, wx.EXPAND)

        lbl_compass_click_level = wx.StaticText(
            sb_compass, label="Compass Click Volume (dBFS):"
        )
        self.spin_compass_click_level = wx.SpinCtrlDouble(
            sb_compass, min=-120.0, max=0.0, inc=1.0
        )
        self.spin_compass_click_level.SetDigits(1)
        self.spin_compass_click_level.SetToolTip(
            "Volume level for compass clicks. 0 is loudest, negatives are quieter."
        )
        self.spin_compass_click_level.SetName("Compass Click Volume")
        _label_spin(self.spin_compass_click_level, "Compass Click Volume")
        compass_grid.Add(lbl_compass_click_level, 0, wx.ALIGN_CENTER_VERTICAL)
        compass_grid.Add(self.spin_compass_click_level, 0, wx.EXPAND)

        lbl_lowspeed_click_level = wx.StaticText(
            sb_compass, label="Low Speed Click Volume (dBFS):"
        )
        self.spin_lowspeed_click_level = wx.SpinCtrlDouble(
            sb_compass, min=-120.0, max=0.0, inc=1.0
        )
        self.spin_lowspeed_click_level.SetDigits(1)
        self.spin_lowspeed_click_level.SetToolTip(
            "Volume level for low speed detection clicks."
        )
        self.spin_lowspeed_click_level.SetName("Low Speed Click Volume")
        _label_spin(self.spin_lowspeed_click_level, "Low Speed Click Volume")
        compass_grid.Add(lbl_lowspeed_click_level, 0, wx.ALIGN_CENTER_VERTICAL)
        compass_grid.Add(self.spin_lowspeed_click_level, 0, wx.EXPAND)

        lbl_lowspeed_stop_level = wx.StaticText(
            sb_compass, label="Low Speed Stop Tone Volume (dBFS):"
        )
        self.spin_lowspeed_stop_level = wx.SpinCtrlDouble(
            sb_compass, min=-120.0, max=0.0, inc=1.0
        )
        self.spin_lowspeed_stop_level.SetDigits(1)
        self.spin_lowspeed_stop_level.SetToolTip(
            "Volume level for the tone that confirms the vehicle has come to a stop."
        )
        self.spin_lowspeed_stop_level.SetName("Low Speed Stop Tone Volume")
        _label_spin(self.spin_lowspeed_stop_level, "Low Speed Stop Tone Volume")
        compass_grid.Add(lbl_lowspeed_stop_level, 0, wx.ALIGN_CENTER_VERTICAL)
        compass_grid.Add(self.spin_lowspeed_stop_level, 0, wx.EXPAND)

        lbl_slip_tone_level = wx.StaticText(
            sb_compass, label="Wheel Slip Tone Volume (dBFS):"
        )
        self.spin_slip_tone_level = wx.SpinCtrlDouble(
            sb_compass, min=-120.0, max=0.0, inc=1.0
        )
        self.spin_slip_tone_level.SetDigits(1)
        self.spin_slip_tone_level.SetToolTip(
            "Volume level for the wheel lockup and wheelspin tone."
        )
        self.spin_slip_tone_level.SetName("Wheel Slip Tone Volume")
        _label_spin(self.spin_slip_tone_level, "Wheel Slip Tone Volume")
        compass_grid.Add(lbl_slip_tone_level, 0, wx.ALIGN_CENTER_VERTICAL)
        compass_grid.Add(self.spin_slip_tone_level, 0, wx.EXPAND)

        lbl_placement_ping_level = wx.StaticText(
            sb_compass, label="Placement Ping Volume (dBFS):"
        )
        self.spin_placement_ping_level = wx.SpinCtrlDouble(
            sb_compass, min=-120.0, max=0.0, inc=1.0
        )
        self.spin_placement_ping_level.SetDigits(1)
        self.spin_placement_ping_level.SetToolTip(
            "Volume level for the movement pings in the vehicle spawner's 3D placement editor."
        )
        self.spin_placement_ping_level.SetName("Placement Ping Volume")
        _label_spin(self.spin_placement_ping_level, "Placement Ping Volume")
        compass_grid.Add(lbl_placement_ping_level, 0, wx.ALIGN_CENTER_VERTICAL)
        compass_grid.Add(self.spin_placement_ping_level, 0, wx.EXPAND)

        compass_sizer.Add(compass_grid, 0, wx.ALL | wx.EXPAND, 6)

        # HRTF controls
        self.chk_hrtf_enabled = wx.CheckBox(sb_compass, label="HRTF Binaural Processing")
        self.chk_hrtf_enabled.SetToolTip(
            "Enable HRTF for 3D spatial audio on compass and low speed clicks. Requires SOFA file."
        )
        compass_sizer.Add(self.chk_hrtf_enabled, 0, wx.LEFT | wx.RIGHT, 6)

        self.hrtf_grid = wx.FlexGridSizer(0, 2, 6, 8)
        self.hrtf_grid.AddGrowableCol(1, 1)

        lbl_hrtf_emphasis = wx.StaticText(sb_compass, label="HRTF Front Emphasis (dB):")
        self.spin_hrtf_front_emphasis = wx.SpinCtrlDouble(
            sb_compass, min=-24.0, max=0.0, inc=1.0
        )
        self.spin_hrtf_front_emphasis.SetDigits(1)
        self.spin_hrtf_front_emphasis.SetToolTip(
            "Attenuation applied to sounds behind you. More negative = rear sounds quieter relative to front."
        )
        self.spin_hrtf_front_emphasis.SetName("HRTF Front Emphasis")
        _label_spin(self.spin_hrtf_front_emphasis, "HRTF Front Emphasis")
        self.hrtf_grid.Add(lbl_hrtf_emphasis, 0, wx.ALIGN_CENTER_VERTICAL)
        self.hrtf_grid.Add(self.spin_hrtf_front_emphasis, 0, wx.EXPAND)

        lbl_hrtf_distance = wx.StaticText(sb_compass, label="HRTF Distance Gain (dB):")
        self.spin_hrtf_distance_gain = wx.SpinCtrlDouble(
            sb_compass, min=-24.0, max=6.0, inc=1.0
        )
        self.spin_hrtf_distance_gain.SetDigits(1)
        self.spin_hrtf_distance_gain.SetToolTip(
            "Additional gain after HRTF processing. Negative values make sounds appear farther away."
        )
        self.spin_hrtf_distance_gain.SetName("HRTF Distance Gain")
        _label_spin(self.spin_hrtf_distance_gain, "HRTF Distance Gain")
        self.hrtf_grid.Add(lbl_hrtf_distance, 0, wx.ALIGN_CENTER_VERTICAL)
        self.hrtf_grid.Add(self.spin_hrtf_distance_gain, 0, wx.EXPAND)

        compass_sizer.Add(self.hrtf_grid, 0, wx.ALL | wx.EXPAND, 6)
        vbox.Add(compass_sizer, 0, wx.LEFT | wx.RIGHT | wx.BOTTOM | wx.EXPAND, 10)

        # --- Vehicle Scanner Group ---
        sb_scanner, scanner_sizer = _group(self, "Vehicle Scanner")

        self.chk_scanner_steer_tone = wx.CheckBox(
            sb_scanner, label="Solid tone while steering"
        )
        self.chk_scanner_steer_tone.SetToolTip(
            "When the scanner is active and you are steering, morph the target beeps into a "
            "continuous directional tone so you can lock onto the target's direction even when "
            "the beeps are slow. Releasing the steering restores the normal beeps."
        )
        scanner_sizer.Add(self.chk_scanner_steer_tone, 0, wx.ALL, 6)

        scanner_grid = wx.FlexGridSizer(0, 2, 6, 8)
        scanner_grid.AddGrowableCol(1, 1)

        lbl_scan_base = wx.StaticText(sb_scanner, label="Base Frequency (Hz):")
        self.spin_scanner_base_freq = wx.SpinCtrl(sb_scanner, min=100, max=8000)
        self.spin_scanner_base_freq.SetToolTip(
            "Resting pitch of the scanner beeps/tone (when the target is directly behind). "
            "The pitch rises toward the target."
        )
        self.spin_scanner_base_freq.SetName("Scanner Base Frequency")
        _label_spin(self.spin_scanner_base_freq, "Scanner Base Frequency")
        scanner_grid.Add(lbl_scan_base, 0, wx.ALIGN_CENTER_VERTICAL)
        scanner_grid.Add(self.spin_scanner_base_freq, 0, wx.EXPAND)

        lbl_scan_offset = wx.StaticText(sb_scanner, label="Alignment Pitch Offset (octaves):")
        self.spin_scanner_offset = wx.SpinCtrlDouble(
            sb_scanner, min=0.5, max=2.0, inc=(1.0 / 12.0)
        )
        self.spin_scanner_offset.SetDigits(2)
        self.spin_scanner_offset.SetToolTip(
            "How far the pitch rises (in octaves) when the target is dead-center, above the base "
            "frequency. Steps by one semitone; minimum half an octave, maximum two octaves."
        )
        self.spin_scanner_offset.SetName("Scanner Alignment Pitch Offset")
        _label_spin(self.spin_scanner_offset, "Scanner Alignment Pitch Offset")
        scanner_grid.Add(lbl_scan_offset, 0, wx.ALIGN_CENTER_VERTICAL)
        scanner_grid.Add(self.spin_scanner_offset, 0, wx.EXPAND)

        scanner_sizer.Add(scanner_grid, 0, wx.LEFT | wx.RIGHT | wx.BOTTOM | wx.EXPAND, 6)
        vbox.Add(scanner_sizer, 0, wx.LEFT | wx.RIGHT | wx.BOTTOM | wx.EXPAND, 10)

        # --- Pitch & Roll Group ---
        # The doubled ampersand is an escaped literal: wx still reads a lone "&"
        # in a label as a mnemonic marker even though this UI no longer uses any.
        sb_pitchroll, pitchroll_sizer = _group(self, "Pitch && Roll")
        sb_pitchroll.SetName("Pitch and Roll")

        self.chk_pitch_roll_enabled = wx.CheckBox(sb_pitchroll, label="Pitch and Roll Tones")
        self.chk_pitch_roll_enabled.SetToolTip(
            "Enable continuous tones that indicate vehicle pitch and roll angle."
        )
        pitchroll_sizer.Add(self.chk_pitch_roll_enabled, 0, wx.ALL, 6)

        pitchroll_grid = wx.FlexGridSizer(0, 2, 6, 8)
        pitchroll_grid.AddGrowableCol(1, 1)

        lbl_pr_max = wx.StaticText(sb_pitchroll, label="Maximum Volume (dBFS):")
        self.spin_pitch_roll_max = wx.SpinCtrlDouble(sb_pitchroll, min=-120.0, max=0.0, inc=1.0)
        self.spin_pitch_roll_max.SetDigits(1)
        self.spin_pitch_roll_max.SetToolTip("Volume level at maximum tilt angle.")
        self.spin_pitch_roll_max.SetName("Pitch Roll Maximum Volume")
        _label_spin(self.spin_pitch_roll_max, "Pitch Roll Maximum Volume")
        pitchroll_grid.Add(lbl_pr_max, 0, wx.ALIGN_CENTER_VERTICAL)
        pitchroll_grid.Add(self.spin_pitch_roll_max, 0, wx.EXPAND)

        lbl_pr_min = wx.StaticText(sb_pitchroll, label="Stabilized Volume (dBFS):")
        self.spin_pitch_roll_min = wx.SpinCtrlDouble(sb_pitchroll, min=-120.0, max=0.0, inc=1.0)
        self.spin_pitch_roll_min.SetDigits(1)
        self.spin_pitch_roll_min.SetToolTip(
            "Volume level when the vehicle is stable (faded down)."
        )
        self.spin_pitch_roll_min.SetName("Pitch Roll Stabilized Volume")
        _label_spin(self.spin_pitch_roll_min, "Pitch Roll Stabilized Volume")
        pitchroll_grid.Add(lbl_pr_min, 0, wx.ALIGN_CENTER_VERTICAL)
        pitchroll_grid.Add(self.spin_pitch_roll_min, 0, wx.EXPAND)

        pitchroll_sizer.Add(
            pitchroll_grid, 0, wx.LEFT | wx.RIGHT | wx.BOTTOM | wx.EXPAND, 6
        )
        vbox.Add(pitchroll_sizer, 0, wx.LEFT | wx.RIGHT | wx.BOTTOM | wx.EXPAND, 10)

        # --- Shift Tone Group ---
        sb_shift, shift_sizer = _group(self, "Shift Tone")
        shift_grid = wx.FlexGridSizer(0, 3, 6, 8)
        shift_grid.AddGrowableCol(1, 1)
        lbl_shift_freq = wx.StaticText(sb_shift, label="Shift Tone Frequency (Hz):")
        self.spin_freq = wx.SpinCtrlDouble(sb_shift, min=20.0, max=20000.0, inc=1.0)
        self.spin_freq.SetDigits(1)
        self.spin_freq.SetToolTip("Triangle tone frequency when shift light is on.")
        self.spin_freq.SetName("Shift Tone Frequency")
        _label_spin(self.spin_freq, "Shift Tone Frequency")
        shift_grid.Add(lbl_shift_freq, 0, wx.ALIGN_CENTER_VERTICAL)
        shift_grid.Add(self.spin_freq, 1, wx.EXPAND)
        shift_grid.Add((0, 0))
        lbl_shift_level = wx.StaticText(sb_shift, label="Shift Tone Level (dBFS):")
        self.spin_level = wx.SpinCtrlDouble(sb_shift, min=-120.0, max=0.0, inc=1.0)
        self.spin_level.SetDigits(1)
        self.spin_level.SetToolTip("Tone level; 0 is loudest, negatives are quieter.")
        self.spin_level.SetName("Shift Tone Level")
        _label_spin(self.spin_level, "Shift Tone Level")
        shift_grid.Add(lbl_shift_level, 0, wx.ALIGN_CENTER_VERTICAL)
        shift_grid.Add(self.spin_level, 1, wx.EXPAND)
        self.btn_test_tone = wx.Button(sb_shift, label="Test Shift Tone")
        self.btn_test_tone.SetToolTip(
            "Plays the shift tone using the current frequency and level settings."
        )
        self.btn_test_tone.Enable(AUDIO_TEST_OK)
        shift_grid.Add(self.btn_test_tone, 0)
        shift_sizer.Add(shift_grid, 0, wx.ALL | wx.EXPAND, 6)
        vbox.Add(shift_sizer, 0, wx.LEFT | wx.RIGHT | wx.BOTTOM | wx.EXPAND, 10)

        # --- Warning Sounds Group ---
        sb_warnings, warnings_sizer = _group(self, "Warning Sounds")

        self.chk_oil_chime_enabled = wx.CheckBox(sb_warnings, label="Oil Overheating Alert")
        self.chk_oil_chime_enabled.SetToolTip(
            "Enable the oil warning chime when the oil pressure warning light is on."
        )
        warnings_sizer.Add(self.chk_oil_chime_enabled, 0, wx.ALL, 6)

        self.chk_tc_clicks_enabled = wx.CheckBox(
            sb_warnings, label="TC (Traction Control) Clicks"
        )
        self.chk_tc_clicks_enabled.SetToolTip(
            "Enable the traction control activation click sound."
        )
        warnings_sizer.Add(
            self.chk_tc_clicks_enabled, 0, wx.LEFT | wx.RIGHT | wx.BOTTOM, 6
        )

        warnings_grid = wx.FlexGridSizer(0, 2, 6, 8)
        warnings_grid.AddGrowableCol(1, 1)

        lbl_buzzer = wx.StaticText(sb_warnings, label="Check Engine Buzzer Level (dBFS):")
        self.spin_buzzer_level = wx.SpinCtrlDouble(sb_warnings, min=-120.0, max=0.0, inc=1.0)
        self.spin_buzzer_level.SetDigits(1)
        self.spin_buzzer_level.SetToolTip("Level for the check engine warning sound.")
        self.spin_buzzer_level.SetName("Check Engine Buzzer Level")
        _label_spin(self.spin_buzzer_level, "Check Engine Buzzer Level")
        warnings_grid.Add(lbl_buzzer, 0, wx.ALIGN_CENTER_VERTICAL)
        warnings_grid.Add(self.spin_buzzer_level, 0, wx.EXPAND)

        lbl_chime = wx.StaticText(sb_warnings, label="Oil Warning Chime Level (dBFS):")
        self.spin_chime_level = wx.SpinCtrlDouble(sb_warnings, min=-120.0, max=0.0, inc=1.0)
        self.spin_chime_level.SetDigits(1)
        self.spin_chime_level.SetToolTip("Level for the oil pressure warning chime.")
        self.spin_chime_level.SetName("Oil Warning Chime Level")
        _label_spin(self.spin_chime_level, "Oil Warning Chime Level")
        warnings_grid.Add(lbl_chime, 0, wx.ALIGN_CENTER_VERTICAL)
        warnings_grid.Add(self.spin_chime_level, 0, wx.EXPAND)

        warnings_sizer.Add(warnings_grid, 0, wx.EXPAND | wx.ALL, 6)
        vbox.Add(warnings_sizer, 0, wx.LEFT | wx.RIGHT | wx.BOTTOM | wx.EXPAND, 10)

        # --- Loader Implement Group ---
        # Only ever audible on a machine with hydraulic implement cylinders (the WL-40
        # wheel loader and anything like it); inert on every ordinary vehicle.
        sb_impl, impl_sizer = _group(self, "Loader Implement")

        self.chk_impl_tones = wx.CheckBox(
            sb_impl, label="Bucket / Fork Tones"
        )
        self.chk_impl_tones.SetToolTip(
            "Ground proximity tone and tilt tone for a loader's bucket or forks. "
            "Both fade out when the controls are idle."
        )
        impl_sizer.Add(self.chk_impl_tones, 0, wx.ALL, 6)

        self.chk_impl_proximity = wx.CheckBox(
            sb_impl, label="Announce Nearby Vehicles and Props"
        )
        self.chk_impl_proximity.SetToolTip(
            "Speak the name of a vehicle or prop the bucket or forks are approaching, "
            "and whether they are above, below or level with it."
        )
        impl_sizer.Add(
            self.chk_impl_proximity, 0, wx.LEFT | wx.RIGHT | wx.BOTTOM, 6
        )

        self.chk_dock_tones = wx.CheckBox(sb_impl, label="Docking Instrument Tones")
        self.chk_dock_tones.SetToolTip(
            "Alignment tones for lining the bucket or forks up with something: a pulse "
            "that pans left and right and speeds up as you close, and a pair of tones "
            "that beat against each other until the implement is level with the "
            "reference band. Only heard while the instrument is switched on in game "
            "with F9 then Ctrl+I."
        )
        impl_sizer.Add(self.chk_dock_tones, 0, wx.LEFT | wx.RIGHT | wx.BOTTOM, 6)

        impl_grid = wx.FlexGridSizer(0, 2, 6, 8)
        impl_grid.AddGrowableCol(1, 1)

        lbl_impl_ground = wx.StaticText(
            sb_impl, label="Ground Proximity Tone Level (dBFS):"
        )
        self.spin_impl_ground = wx.SpinCtrlDouble(sb_impl, min=-120.0, max=0.0, inc=1.0)
        self.spin_impl_ground.SetDigits(1)
        self.spin_impl_ground.SetToolTip(
            "Level of the tone that gets rougher as the bucket or forks near the ground."
        )
        self.spin_impl_ground.SetName("Ground Proximity Tone Level")
        _label_spin(self.spin_impl_ground, "Ground Proximity Tone Level")
        impl_grid.Add(lbl_impl_ground, 0, wx.ALIGN_CENTER_VERTICAL)
        impl_grid.Add(self.spin_impl_ground, 0, wx.EXPAND)

        lbl_impl_tilt = wx.StaticText(sb_impl, label="Tilt Tone Level (dBFS):")
        self.spin_impl_tilt = wx.SpinCtrlDouble(sb_impl, min=-120.0, max=0.0, inc=1.0)
        self.spin_impl_tilt.SetDigits(1)
        self.spin_impl_tilt.SetToolTip(
            "Level of the quarter-tone tilt scale. 400 Hz is level, lower is tipped "
            "forward, higher is curled back."
        )
        self.spin_impl_tilt.SetName("Tilt Tone Level")
        _label_spin(self.spin_impl_tilt, "Tilt Tone Level")
        impl_grid.Add(lbl_impl_tilt, 0, wx.ALIGN_CENTER_VERTICAL)
        impl_grid.Add(self.spin_impl_tilt, 0, wx.EXPAND)

        lbl_dock = wx.StaticText(sb_impl, label="Docking Tone Level (dBFS):")
        self.spin_dock = wx.SpinCtrlDouble(sb_impl, min=-120.0, max=0.0, inc=1.0)
        self.spin_dock.SetDigits(1)
        self.spin_dock.SetToolTip(
            "Level of the docking pulse. The beating alignment pair is fixed relative to "
            "it, so that the two stay separable."
        )
        self.spin_dock.SetName("Docking Tone Level")
        _label_spin(self.spin_dock, "Docking Tone Level")
        impl_grid.Add(lbl_dock, 0, wx.ALIGN_CENTER_VERTICAL)
        impl_grid.Add(self.spin_dock, 0, wx.EXPAND)

        impl_sizer.Add(impl_grid, 0, wx.EXPAND | wx.ALL, 6)
        vbox.Add(impl_sizer, 0, wx.LEFT | wx.RIGHT | wx.BOTTOM | wx.EXPAND, 10)

        # --- Terrain Scanner Group ---
        sb_scan, scan_sizer = _group(self, "Terrain Scanner")

        self.chk_scan_tones = wx.CheckBox(sb_scan, label="Terrain Scan Tones")
        self.chk_scan_tones.SetToolTip(
            "The terrain sonification scan, fired in game with F9 then Space while "
            "you are driving. Plays a reference tone and then a two second sweep of the "
            "ground ahead, where "
            "pitch is height, time is distance and the stereo position is the direction. "
            "Water is brighter, and vehicles and props are short bright pings."
        )
        scan_sizer.Add(self.chk_scan_tones, 0, wx.ALL, 6)

        scan_grid = wx.FlexGridSizer(0, 2, 6, 8)
        scan_grid.AddGrowableCol(1, 1)

        lbl_scan = wx.StaticText(sb_scan, label="Terrain Scan Level (dBFS):")
        self.spin_scan = wx.SpinCtrlDouble(sb_scan, min=-120.0, max=0.0, inc=1.0)
        self.spin_scan.SetDigits(1)
        self.spin_scan.SetToolTip(
            "Level of the whole scan. Water and the vehicle pings sit at fixed offsets "
            "from it, so they stay in proportion at any setting."
        )
        self.spin_scan.SetName("Terrain Scan Level")
        _label_spin(self.spin_scan, "Terrain Scan Level")
        scan_grid.Add(lbl_scan, 0, wx.ALIGN_CENTER_VERTICAL)
        scan_grid.Add(self.spin_scan, 0, wx.EXPAND)

        scan_sizer.Add(scan_grid, 0, wx.EXPAND | wx.ALL, 6)
        vbox.Add(scan_sizer, 0, wx.LEFT | wx.RIGHT | wx.BOTTOM | wx.EXPAND, 10)

        # --- Large Cannon Group ---
        sb_cannon, cannon_sizer = _group(self, "Large Cannon")

        self.chk_cannon_shot = wx.CheckBox(sb_cannon, label="Announce Shot Outcome")
        self.chk_cannon_shot.SetToolTip(
            "After a car is fired out of the large cannon, speak where it ended up once it "
            "comes to rest: how far downrange it went, how far off the firing line, and how "
            "much further or shorter than your previous shot. There is no key to press; the "
            "readout announces itself, because the moment the car stops is the only moment it "
            "has anything to say."
        )
        cannon_sizer.Add(self.chk_cannon_shot, 0, wx.ALL, 6)
        vbox.Add(cannon_sizer, 0, wx.LEFT | wx.RIGHT | wx.BOTTOM | wx.EXPAND, 10)

        # --- Audio Device Group ---
        sb_audio, audio_sizer = _group(self, "Audio Device")

        self.chk_follow_device = wx.CheckBox(sb_audio, label="Follow Default Audio Device")
        self.chk_follow_device.SetToolTip(
            "Automatically switch the audio output when the OS default device changes (recommended)."
        )
        audio_sizer.Add(self.chk_follow_device, 0, wx.ALL, 6)

        # Fixed-device row — shown only when "Follow" is unchecked.
        self._device_row = wx.BoxSizer(wx.HORIZONTAL)
        lbl_fixed = wx.StaticText(sb_audio, label="Fixed Output Device:")
        self.choice_device = wx.Choice(sb_audio)
        self.choice_device.SetName("Fixed Output Device")
        self.choice_device.SetToolTip(
            "WASAPI output device to use when not following the OS default."
        )
        btn_refresh_device = wx.Button(sb_audio, label="Refresh Devices")
        btn_refresh_device.SetToolTip("Re-scan for WASAPI output devices.")
        self._device_row.Add(lbl_fixed, 0, wx.ALIGN_CENTER_VERTICAL | wx.RIGHT, 6)
        self._device_row.Add(
            self.choice_device, 1, wx.ALIGN_CENTER_VERTICAL | wx.RIGHT, 4
        )
        self._device_row.Add(btn_refresh_device, 0, wx.ALIGN_CENTER_VERTICAL)
        audio_sizer.Add(
            self._device_row, 0, wx.EXPAND | wx.LEFT | wx.RIGHT | wx.BOTTOM, 6
        )

        poll_row = wx.FlexGridSizer(0, 2, 6, 8)
        poll_row.AddGrowableCol(1, 1)
        lbl_poll = wx.StaticText(sb_audio, label="Device Poll Interval (sec):")
        self.spin_poll = wx.SpinCtrlDouble(sb_audio, min=0.1, max=10.0, inc=0.1)
        self.spin_poll.SetDigits(1)
        self.spin_poll.SetName("Device Poll Interval")
        _label_spin(self.spin_poll, "Device Poll Interval")
        self.spin_poll.SetToolTip(
            "How often to check for default audio device changes (in seconds)."
        )
        poll_row.Add(lbl_poll, 0, wx.ALIGN_CENTER_VERTICAL)
        poll_row.Add(self.spin_poll, 1, wx.EXPAND)
        audio_sizer.Add(poll_row, 0, wx.EXPAND | wx.LEFT | wx.RIGHT | wx.BOTTOM, 6)

        vbox.Add(audio_sizer, 0, wx.LEFT | wx.RIGHT | wx.BOTTOM | wx.EXPAND, 10)

        # --- Developer Group ---
        sb_dev, dev_sizer = _group(self, "Developer")

        self.chk_mcp_server = wx.CheckBox(
            sb_dev, label="Enable MCP automation server"
        )
        self.chk_mcp_server.SetToolTip(
            "Lets an AI assistant running on this computer drive the mod and execute "
            "code inside BeamNG.drive, so it can test features without you having to "
            "set each one up by hand. Listens on this machine only. Leave this off "
            "unless you are actively working with an assistant. Takes effect the next "
            "time BeamTel starts."
        )
        dev_sizer.Add(self.chk_mcp_server, 0, wx.ALL, 6)

        mcp_row = wx.FlexGridSizer(0, 2, 6, 8)
        mcp_row.AddGrowableCol(1, 1)
        lbl_mcp_port = wx.StaticText(sb_dev, label="MCP Server Port:")
        self.spin_mcp_port = wx.SpinCtrl(sb_dev, min=1024, max=65535)
        self.spin_mcp_port.SetName("MCP Server Port")
        _label_spin(self.spin_mcp_port, "MCP Server Port")
        self.spin_mcp_port.SetToolTip(
            "Which local port the automation server listens on. Change this only if "
            "another program already uses it."
        )
        mcp_row.Add(lbl_mcp_port, 0, wx.ALIGN_CENTER_VERTICAL)
        mcp_row.Add(self.spin_mcp_port, 1, wx.EXPAND)
        dev_sizer.Add(mcp_row, 0, wx.EXPAND | wx.LEFT | wx.RIGHT | wx.BOTTOM, 6)

        vbox.Add(dev_sizer, 0, wx.LEFT | wx.RIGHT | wx.BOTTOM | wx.EXPAND, 10)

        # --- Buttons ---
        btn_sizer = wx.BoxSizer(wx.HORIZONTAL)
        self.btn_reset = wx.Button(self, label="Reset to Defaults")
        btn_sizer.Add(self.btn_reset, 0, wx.ALL, 5)
        vbox.Add(btn_sizer, 0, wx.ALIGN_RIGHT | wx.RIGHT | wx.BOTTOM, 6)

        self.SetSizer(vbox)

        # Events — toggle handlers
        self.chk_compass_highlight.Bind(wx.EVT_CHECKBOX, self.on_toggle_highlight)
        self.chk_hrtf_enabled.Bind(wx.EVT_CHECKBOX, self.on_toggle_hrtf)
        self.chk_follow_device.Bind(wx.EVT_CHECKBOX, self.on_toggle_follow_device)
        self.chk_announce_speed.Bind(wx.EVT_CHECKBOX, self.on_toggle_announce_speed)
        self.chk_launch_beamng.Bind(wx.EVT_CHECKBOX, self.on_toggle_launch_beamng)
        self.chk_scanner_callout.Bind(wx.EVT_CHECKBOX, self.on_toggle_scanner_callout)
        btn_refresh.Bind(wx.EVT_BUTTON, self.on_refresh_voices)
        btn_refresh_device.Bind(wx.EVT_BUTTON, self.on_refresh_devices)
        btn_test.Bind(wx.EVT_BUTTON, self.on_test_voice)
        self.btn_test_tone.Bind(wx.EVT_BUTTON, self.on_test_tone)
        self.btn_reset.Bind(wx.EVT_BUTTON, self.on_reset)

        # Auto-save: bind all data controls
        for ctrl in (
            self.chk_launch_beamng,
            self.chk_update_check,
            self.chk_compass_highlight,
            self.chk_hrtf_enabled,
            self.chk_pitch_roll_enabled,
            self.chk_oil_chime_enabled,
            self.chk_tc_clicks_enabled,
            self.chk_follow_device,
            self.chk_announce_turn_signals,
            self.chk_announce_speed,
            self.chk_announce_gear,
            self.chk_scanner_callout,
            self.chk_scanner_steer_tone,
            self.chk_ui_nav_hold,
            self.chk_impl_tones,
            self.chk_impl_proximity,
            self.chk_dock_tones,
            self.chk_scan_tones,
            self.chk_cannon_shot,
            self.chk_mcp_server,
        ):
            ctrl.Bind(wx.EVT_CHECKBOX, self._schedule_save)

        self.choice_device.Bind(wx.EVT_CHOICE, self._schedule_save)
        self.choice_speed_interval.Bind(wx.EVT_CHOICE, self._schedule_save)
        self.choice_renderer.Bind(wx.EVT_CHOICE, self._schedule_save)
        self.choice_callout_interval.Bind(wx.EVT_CHOICE, self._schedule_save)

        for ctrl in (self.rb_units, self.rb_proto):
            ctrl.Bind(wx.EVT_RADIOBOX, self._schedule_save)
        self.rb_units.Bind(wx.EVT_RADIOBOX, self._on_units_changed)

        for ctrl in (
            self.spin_rate,
            self.spin_volume,
            self.spin_compass_interval,
            self.spin_compass_highlight_nth,
            self.spin_scanner_base_freq,
            self.spin_mcp_port,
        ):
            ctrl.Bind(wx.EVT_SPINCTRL, self._schedule_save)

        for ctrl in (
            self.spin_freq,
            self.spin_level,
            self.spin_buzzer_level,
            self.spin_chime_level,
            self.spin_impl_ground,
            self.spin_impl_tilt,
            self.spin_dock,
            self.spin_scan,
            self.spin_compass_click_level,
            self.spin_lowspeed_click_level,
            self.spin_lowspeed_stop_level,
            self.spin_slip_tone_level,
            self.spin_placement_ping_level,
            self.spin_hrtf_front_emphasis,
            self.spin_hrtf_distance_gain,
            self.spin_pitch_roll_max,
            self.spin_pitch_roll_min,
            self.spin_poll,
            self.spin_scanner_offset,
        ):
            ctrl.Bind(wx.EVT_SPINCTRLDOUBLE, self._schedule_save)
            _bind_spin_double_page_keys(ctrl, self._schedule_save)

        self.choice_voice.Bind(wx.EVT_CHOICE, self._schedule_save)
        self.choice_backend.Bind(wx.EVT_CHOICE, self.on_change_backend)

        # Populate
        self.voices = []
        self._backends = []
        self._callout_intervals = [5, 10, 15, 20, 30, 45, 60]
        self.choice_callout_interval.AppendItems(
            [f"{v} seconds" for v in self._callout_intervals]
        )
        self.on_refresh_devices(None)  # fill device list before load_into_controls
        self._populate_backends()  # same, for the speech backend list
        self.load_into_controls(load_config())
        self.on_toggle_highlight(None)
        self.on_toggle_hrtf(None)
        self.on_toggle_follow_device(None)
        self.on_toggle_announce_speed(None)
        self.on_toggle_launch_beamng(None)
        self.on_toggle_scanner_callout(None)
        self.on_refresh_voices(None)

        # Every group's controls are parented to its StaticBox (see _group), so
        # the box supplies the group name from its own label -- no SetName
        # needed here.  The one exception is Pitch && Roll, named at creation
        # because its label carries an escaped ampersand.

    # ---- Toggle handlers ----

    def on_toggle_highlight(self, evt):
        # ShowItems, not Show: wx.Sizer.Show with a single argument binds to the
        # Show(index, ...) overload, so Show(True)/Show(False) silently meant
        # "show item 1" / "show item 0" and the row never hid at all.
        _set_row(
            self._highlight_row,
            (self.spin_compass_highlight_nth,),
            self.chk_compass_highlight.IsChecked(),
            self.chk_compass_highlight,
        )
        self.Layout()

    def on_toggle_hrtf(self, evt):
        _set_row(
            self.hrtf_grid,
            (self.spin_hrtf_front_emphasis, self.spin_hrtf_distance_gain),
            self.chk_hrtf_enabled.IsChecked(),
            self.chk_hrtf_enabled,
        )
        self.Layout()

    def on_toggle_announce_speed(self, evt):
        _set_row(
            self._speed_interval_sizer,
            (self.choice_speed_interval,),
            self.chk_announce_speed.IsChecked(),
            self.chk_announce_speed,
        )
        self.Layout()
        if evt:
            evt.Skip()

    def _update_speed_interval_labels(self):
        unit = "km/h" if self.rb_units.GetSelection() == 1 else "mph"
        intervals = [25, 50, 75, 100]
        sel = self.choice_speed_interval.GetSelection()
        self.choice_speed_interval.Clear()
        for v in intervals:
            self.choice_speed_interval.Append(f"Every {v} {unit}")
        self.choice_speed_interval.SetSelection(max(0, sel if sel != wx.NOT_FOUND else 0))

    def _on_units_changed(self, evt):
        self._update_speed_interval_labels()
        if evt:
            evt.Skip()

    # ---- Auto-save ----

    def _schedule_save(self, evt=None):
        """Reset the 2-second auto-save countdown on any UI change."""
        if self._loading:
            if evt:
                evt.Skip()
            return
        self._save_timer.StartOnce(_AUTO_SAVE_DELAY_MS)
        if evt:
            evt.Skip()

    def _auto_save(self, evt=None):
        """Write current controls to the config file (called by the timer)."""
        try:
            cfg = self.controls_to_config()
            # The AI Describer tab owns these keys and writes them independently;
            # re-read them from disk so this panel's snapshot doesn't clobber a
            # value the user just changed there.
            try:
                import ai_describer

                disk = load_config()
                for k in ai_describer.all_config_keys():
                    if k in disk:
                        cfg[k] = disk[k]
                # Same argument, other owner: updater.py writes
                # pending_update_version behind this panel's back, and
                # controls_to_config() seeds from a snapshot taken when the panel
                # was built. Without this, an auto-save that happens to land
                # between the update being staged and the restart would wipe the
                # one fact that has to survive it.
                if "pending_update_version" in disk:
                    cfg["pending_update_version"] = disk["pending_update_version"]
            except Exception:
                pass
            _write_config(CONFIG_PATH, cfg)
            self.cur_cfg = cfg
        except Exception:
            pass

    def flush_pending_save(self):
        """Write immediately if the debounce timer is still counting down.

        Saving is debounced by 2 seconds, so closing the window straight after
        an edit would otherwise discard it with no indication at all. Call this
        from the owning frame's close handler.
        """
        if self._save_timer.IsRunning():
            self._save_timer.Stop()
            self._auto_save()

    # ---- Config <-> Controls ----

    def load_into_controls(self, cfg):
        self._loading = True
        try:
            self.cur_cfg = cfg
            self._select_backend(cfg.get("speech_backend", "auto"))
            self.rb_units.SetSelection(
                1 if str(cfg.get("units", "imperial")).lower().startswith("m") else 0
            )
            self.rb_proto.SetSelection(
                1
                if str(cfg.get("telemetry_protocol", "extended")).lower() == "outgauge"
                else 0
            )
            self.spin_rate.SetValue(cfg.get("speech_rate", 50))
            self.spin_volume.SetValue(cfg.get("speech_volume", 100))
            self.spin_freq.SetValue(cfg.get("shift_tone_frequency_hz", 880.0))
            self.spin_level.SetValue(cfg.get("shift_tone_level_dbfs", -12.0))
            self.spin_buzzer_level.SetValue(
                cfg.get("check_engine_buzzer_level_dbfs", -12.0)
            )
            self.spin_chime_level.SetValue(cfg.get("oil_chime_level_dbfs", -12.0))

            self.chk_impl_tones.SetValue(cfg.get("implement_tones_enabled", True))
            self.chk_impl_proximity.SetValue(
                cfg.get("implement_proximity_speech", True)
            )
            self.spin_impl_ground.SetValue(
                cfg.get("implement_ground_tone_dbfs", -19.5)
            )
            self.spin_impl_tilt.SetValue(cfg.get("implement_tilt_tone_dbfs", -20.0))
            self.chk_dock_tones.SetValue(cfg.get("dock_tones_enabled", True))
            self.chk_scan_tones.SetValue(cfg.get("scan_tones_enabled", True))
            self.chk_cannon_shot.SetValue(cfg.get("cannon_shot_readout", True))
            self.spin_dock.SetValue(cfg.get("dock_tone_dbfs", -18.0))
            self.spin_scan.SetValue(cfg.get("scan_tone_dbfs", -20.0))

            self.spin_compass_interval.SetValue(cfg.get("compass_click_interval", 15))
            self.chk_compass_highlight.SetValue(
                cfg.get("compass_highlight_enabled", False)
            )
            self.spin_compass_highlight_nth.SetValue(
                cfg.get("compass_highlight_nth_click", 6)
            )
            self.spin_compass_click_level.SetValue(
                cfg.get("compass_click_level_dbfs", -6.0)
            )
            self.spin_lowspeed_click_level.SetValue(
                cfg.get("lowspeed_click_level_dbfs", -14.0)
            )
            self.spin_lowspeed_stop_level.SetValue(
                cfg.get("lowspeed_stop_tone_level_dbfs", -16.0)
            )
            self.spin_slip_tone_level.SetValue(cfg.get("slip_tone_level_dbfs", -18.0))
            self.spin_placement_ping_level.SetValue(
                cfg.get("placement_ping_volume_db", -12.0)
            )
            self.chk_hrtf_enabled.SetValue(cfg.get("hrtf_enabled", True))
            self.spin_hrtf_front_emphasis.SetValue(
                cfg.get("hrtf_front_emphasis_db", -6.0)
            )
            self.spin_hrtf_distance_gain.SetValue(cfg.get("hrtf_distance_gain_db", 0.0))
            self.chk_pitch_roll_enabled.SetValue(
                cfg.get("pitch_roll_tones_enabled", True)
            )
            self.spin_pitch_roll_max.SetValue(cfg.get("pitch_roll_max_dbfs", -24.0))
            self.spin_pitch_roll_min.SetValue(cfg.get("pitch_roll_min_dbfs", -36.0))
            self.chk_oil_chime_enabled.SetValue(cfg.get("oil_chime_enabled", True))
            self.chk_tc_clicks_enabled.SetValue(cfg.get("tc_clicks_enabled", True))
            self.chk_launch_beamng.SetValue(cfg.get("launch_beamng", False))
            self.chk_update_check.SetValue(cfg.get("update_check_enabled", True))
            renderer = str(cfg.get("beamng_renderer", "d3d")).lower()
            self.choice_renderer.SetSelection(1 if renderer == "vulkan" else 0)
            self.chk_follow_device.SetValue(
                cfg.get("follow_default_audio_device", True)
            )
            self.spin_poll.SetValue(cfg.get("audio_poll_interval_sec", 2.0))
            self.chk_announce_turn_signals.SetValue(cfg.get("announce_turn_signals", True))
            self.chk_announce_speed.SetValue(cfg.get("announce_speed", True))
            self.chk_announce_gear.SetValue(cfg.get("announce_gear", True))
            self.chk_scanner_callout.SetValue(cfg.get("scanner_distance_callout_enabled", False))
            callout_val = cfg.get("scanner_distance_callout_interval", 10)
            callout_idx = self._callout_intervals.index(callout_val) if callout_val in self._callout_intervals else 1
            self.choice_callout_interval.SetSelection(callout_idx)
            self.chk_scanner_steer_tone.SetValue(cfg.get("scanner_steer_tone_enabled", True))
            self.spin_scanner_base_freq.SetValue(int(round(cfg.get("scanner_base_freq_hz", 1000.0))))
            self.spin_scanner_offset.SetValue(cfg.get("scanner_pitch_offset_oct", 1.0))
            self.chk_ui_nav_hold.SetValue(cfg.get("ui_nav_hold_suppression", True))
            self.chk_mcp_server.SetValue(cfg.get("mcp_server_enabled", False))
            self.spin_mcp_port.SetValue(int(cfg.get("mcp_server_port", 4481)))
            self._update_speed_interval_labels()
            interval_val = cfg.get("speed_announce_interval", 25)
            interval_choices = [25, 50, 75, 100]
            idx = interval_choices.index(interval_val) if interval_val in interval_choices else 0
            self.choice_speed_interval.SetSelection(idx)
            # Select the saved fixed device in the choice, if present
            want = str(cfg.get("preferred_device_name", "")).strip()
            idx = self.choice_device.FindString(want)
            self.choice_device.SetSelection(idx if idx != wx.NOT_FOUND else 0)
        finally:
            self._loading = False

    def controls_to_config(self):
        cfg = self.cur_cfg.copy()
        cfg["speech_backend"] = self._selected_backend()
        cfg["units"] = "metric" if self.rb_units.GetSelection() == 1 else "imperial"
        cfg["telemetry_protocol"] = (
            "outgauge" if self.rb_proto.GetSelection() == 1 else "extended"
        )
        cfg["speech_rate"] = self.spin_rate.GetValue()
        cfg["speech_volume"] = self.spin_volume.GetValue()
        cfg["shift_tone_frequency_hz"] = self.spin_freq.GetValue()
        cfg["shift_tone_level_dbfs"] = self.spin_level.GetValue()
        cfg["check_engine_buzzer_level_dbfs"] = self.spin_buzzer_level.GetValue()
        cfg["oil_chime_level_dbfs"] = self.spin_chime_level.GetValue()

        cfg["implement_tones_enabled"] = self.chk_impl_tones.GetValue()
        cfg["implement_proximity_speech"] = self.chk_impl_proximity.GetValue()
        cfg["implement_ground_tone_dbfs"] = self.spin_impl_ground.GetValue()
        cfg["implement_tilt_tone_dbfs"] = self.spin_impl_tilt.GetValue()
        cfg["dock_tones_enabled"] = self.chk_dock_tones.GetValue()
        cfg["scan_tones_enabled"] = self.chk_scan_tones.GetValue()
        cfg["cannon_shot_readout"] = self.chk_cannon_shot.GetValue()
        cfg["dock_tone_dbfs"] = self.spin_dock.GetValue()
        cfg["scan_tone_dbfs"] = self.spin_scan.GetValue()

        cfg["compass_click_interval"] = self.spin_compass_interval.GetValue()
        cfg["compass_highlight_enabled"] = self.chk_compass_highlight.GetValue()
        cfg["compass_highlight_nth_click"] = self.spin_compass_highlight_nth.GetValue()
        cfg["compass_click_level_dbfs"] = self.spin_compass_click_level.GetValue()
        cfg["lowspeed_click_level_dbfs"] = self.spin_lowspeed_click_level.GetValue()
        cfg["lowspeed_stop_tone_level_dbfs"] = self.spin_lowspeed_stop_level.GetValue()
        cfg["slip_tone_level_dbfs"] = self.spin_slip_tone_level.GetValue()
        cfg["placement_ping_volume_db"] = self.spin_placement_ping_level.GetValue()
        cfg["hrtf_enabled"] = self.chk_hrtf_enabled.GetValue()
        cfg["hrtf_front_emphasis_db"] = self.spin_hrtf_front_emphasis.GetValue()
        cfg["hrtf_distance_gain_db"] = self.spin_hrtf_distance_gain.GetValue()
        cfg["pitch_roll_tones_enabled"] = self.chk_pitch_roll_enabled.GetValue()
        cfg["pitch_roll_max_dbfs"] = self.spin_pitch_roll_max.GetValue()
        cfg["pitch_roll_min_dbfs"] = self.spin_pitch_roll_min.GetValue()
        cfg["oil_chime_enabled"] = self.chk_oil_chime_enabled.GetValue()
        cfg["tc_clicks_enabled"] = self.chk_tc_clicks_enabled.GetValue()
        cfg["launch_beamng"] = self.chk_launch_beamng.GetValue()
        cfg["update_check_enabled"] = self.chk_update_check.GetValue()
        cfg["beamng_renderer"] = "vulkan" if self.choice_renderer.GetSelection() == 1 else "d3d"
        cfg["follow_default_audio_device"] = self.chk_follow_device.GetValue()
        cfg["audio_poll_interval_sec"] = self.spin_poll.GetValue()
        cfg["announce_turn_signals"] = self.chk_announce_turn_signals.GetValue()
        cfg["announce_speed"] = self.chk_announce_speed.GetValue()
        cfg["announce_gear"] = self.chk_announce_gear.GetValue()
        cfg["scanner_distance_callout_enabled"] = self.chk_scanner_callout.GetValue()
        ci = self.choice_callout_interval.GetSelection()
        cfg["scanner_distance_callout_interval"] = self._callout_intervals[ci] if 0 <= ci < len(self._callout_intervals) else 10
        cfg["scanner_steer_tone_enabled"] = self.chk_scanner_steer_tone.GetValue()
        cfg["scanner_base_freq_hz"] = float(self.spin_scanner_base_freq.GetValue())
        # Snap the octave offset to whole semitones so it stays on a musical grid.
        cfg["scanner_pitch_offset_oct"] = round(self.spin_scanner_offset.GetValue() * 12.0) / 12.0
        cfg["ui_nav_hold_suppression"] = self.chk_ui_nav_hold.GetValue()
        cfg["mcp_server_enabled"] = self.chk_mcp_server.GetValue()
        cfg["mcp_server_port"] = int(self.spin_mcp_port.GetValue())
        interval_choices = [25, 50, 75, 100]
        sel_idx = self.choice_speed_interval.GetSelection()
        cfg["speed_announce_interval"] = interval_choices[sel_idx] if 0 <= sel_idx < len(interval_choices) else 25
        sel = self.choice_device.GetSelection()
        cfg["preferred_device_name"] = (
            self.choice_device.GetString(sel) if sel != wx.NOT_FOUND else ""
        )

        sel = self.choice_voice.GetSelection()
        cfg["speech_voice_name"] = (
            self.choice_voice.GetString(sel)
            if sel != wx.NOT_FOUND and self.voices
            else ""
        )
        return cfg

    # ---- Speech backend helpers ----

    _AUTO_BACKEND_LABEL = "Auto (best available)"

    def _populate_backends(self):
        """Fill the backend picker from Prism's registry."""
        self._backends = list_speech_backends()
        self.choice_backend.Clear()
        self.choice_backend.AppendItems(
            [self._AUTO_BACKEND_LABEL] + self._backends
        )
        self.choice_backend.SetSelection(0)

    def _select_backend(self, name):
        want = (name or "auto").strip()
        idx = 0
        if want.lower() != "auto":
            for i, nm in enumerate(self._backends):
                if nm.lower() == want.lower():
                    idx = i + 1  # +1 for the Auto entry
                    break
        self.choice_backend.SetSelection(idx)

    def _selected_backend(self):
        sel = self.choice_backend.GetSelection()
        if sel <= 0 or sel - 1 >= len(self._backends):
            return "auto"
        return self._backends[sel - 1]

    def _apply_backend_capabilities(self, feats):
        """Grey out the controls the selected backend does not implement.

        Screen readers own their own voice and rate, so those controls are
        inert for NVDA or JAWS; disabling them is more honest than accepting a
        value that will be ignored.
        """
        if feats is None:
            for ctrl in (self.choice_voice, self.spin_rate, self.spin_volume):
                _enable(ctrl, False, self.choice_backend)
            self.lbl_speech_caps.SetLabel("Backend not available on this machine.")
            return
        _enable(self.choice_voice, bool(feats.supports_set_voice), self.choice_backend)
        _enable(self.spin_rate, bool(feats.supports_set_rate), self.choice_backend)
        _enable(self.spin_volume, bool(feats.supports_set_volume), self.choice_backend)
        can = [
            label
            for label, ok in (
                ("voice", feats.supports_set_voice),
                ("rate", feats.supports_set_rate),
                ("volume", feats.supports_set_volume),
                ("braille", feats.supports_braille),
            )
            if ok
        ]
        self.lbl_speech_caps.SetLabel(
            "This backend supports: " + ", ".join(can)
            if can
            else "This backend manages its own voice, rate, and volume."
        )

    def on_change_backend(self, evt):
        self.on_refresh_voices(None)
        self._schedule_save(evt)

    # ---- Button handlers ----

    def on_refresh_voices(self, evt):
        if evt is not None:
            # An explicit Refresh should also re-detect readers that started
            # since the configurator opened.
            current = self._selected_backend()
            self._populate_backends()
            self._select_backend(current)
        names, feats = list_speech_voices(self._selected_backend())
        self.voices = names[:]
        self.choice_voice.Clear()
        if names:
            self.choice_voice.AppendItems(names)
            want = (self.cur_cfg.get("speech_voice_name", "") or "").strip().lower()
            sel_idx = 0
            if want:
                for i, nm in enumerate(names):
                    if nm.strip().lower() == want:
                        sel_idx = i
                        break
            self.choice_voice.SetSelection(sel_idx)
        else:
            self.choice_voice.Append("Backend chooses its own voice")
            self.choice_voice.SetSelection(0)
        self._apply_backend_capabilities(feats)

    def on_test_voice(self, evt):
        cfg = self.controls_to_config()
        ok, err = test_speech_voice(
            cfg.get("speech_backend", "auto"),
            cfg.get("speech_voice_name", ""),
            cfg.get("speech_rate", 50),
            cfg.get("speech_volume", 100),
        )
        if not ok and err:
            wx.MessageBox(err, "Speech Test", wx.OK | wx.ICON_ERROR)

    def on_test_tone(self, evt):
        freq = self.spin_freq.GetValue()
        level = self.spin_level.GetValue()
        ok, err = play_test_tone(freq, level)
        if not ok and err:
            wx.MessageBox(err, "Audio Error", wx.OK | wx.ICON_ERROR)

    def on_toggle_launch_beamng(self, evt):
        """Show the renderer dropdown only when auto-launch is enabled."""
        _set_row(
            self._renderer_row,
            (self.choice_renderer,),
            self.chk_launch_beamng.IsChecked(),
            self.chk_launch_beamng,
        )
        self.Layout()
        if evt:
            evt.Skip()

    def on_toggle_scanner_callout(self, evt):
        """Show the interval dropdown only when scanner callouts are enabled."""
        _set_row(
            self._callout_interval_sizer,
            (self.choice_callout_interval,),
            self.chk_scanner_callout.IsChecked(),
            self.chk_scanner_callout,
        )
        self.Layout()
        if evt:
            evt.Skip()

    def on_toggle_follow_device(self, evt):
        """Show the fixed-device row only when 'Follow Default' is unchecked."""
        # Match the other four toggles: hiding alone leaves the control out of
        # the tab order, but disabling it keeps the two states consistent.
        _set_row(
            self._device_row,
            (self.choice_device,),
            not self.chk_follow_device.IsChecked(),
            self.chk_follow_device,
        )
        self.Layout()
        if evt:
            evt.Skip()

    def on_refresh_devices(self, evt):
        """Re-populate the WASAPI output device choice."""
        names = list_wasapi_output_devices()
        prev = self.choice_device.GetStringSelection()
        self.choice_device.Clear()
        if names:
            self.choice_device.AppendItems(names)
            idx = self.choice_device.FindString(prev)
            self.choice_device.SetSelection(idx if idx != wx.NOT_FOUND else 0)
        else:
            self.choice_device.Append("(no WASAPI devices found)")
            self.choice_device.SetSelection(0)

    def on_reset(self, evt):
        # Destructive and irreversible, and there is no save button to hesitate
        # over -- the wipe reaches disk on its own two seconds later. Mirror
        # _clear_api_key and ask first.
        ans = wx.MessageBox(
            "Reset every setting on this tab to its default value?",
            "Reset to Defaults",
            wx.YES_NO | wx.NO_DEFAULT | wx.ICON_QUESTION,
            self,
        )
        if ans != wx.YES:
            return
        try:
            self.load_into_controls(DEFAULT_CONFIG.copy())
            self.on_refresh_voices(None)
            self.on_refresh_devices(None)
            self.on_toggle_highlight(None)
            self.on_toggle_hrtf(None)
            self.on_toggle_follow_device(None)
            self.on_toggle_launch_beamng(None)
            self.on_toggle_announce_speed(None)
            self.on_toggle_scanner_callout(None)
            self._schedule_save()
        except Exception as e:
            wx.MessageBox(f"Failed to reset:\n{e}", "Error", wx.OK | wx.ICON_ERROR)
            return
        # Acknowledge the destructive action; without this there is no signal at
        # all that the reset happened.
        wx.MessageBox(
            "All settings on this tab have been reset to their defaults.",
            "Reset to Defaults",
            wx.OK | wx.ICON_INFORMATION,
            self,
        )


class AIDescriberPanel(wx.ScrolledWindow):
    """Configuration for the AI Describer feature (AI scene description).

    The provider dropdown drives everything below it: the API key group is
    retitled, the model list is repopulated, and each provider's extra request
    parameters (declared in ai_describer's registry) are shown or hidden. Every
    provider keeps its own key and model on disk, so switching back and forth
    never loses a setting.

    Writes are merged onto a fresh disk read so this panel doesn't clobber
    settings owned by the main Configuration tab.
    """

    def __init__(self, parent):
        super().__init__(parent, style=wx.TAB_TRAVERSAL)
        self.SetScrollRate(0, 10)
        self.cur_cfg = load_config()

        self.Bind(wx.EVT_NAVIGATION_KEY, lambda evt: wrap_nav_key(evt, self))

        import ai_describer

        self._ai = ai_describer
        self._providers = list(ai_describer.PROVIDERS)
        self._provider = ai_describer.DEFAULT_PROVIDER
        self._models = list(ai_describer.vision_models_for(self._provider))

        # The base-URL field saves on a debounce rather than per keystroke;
        # every other control here still saves eagerly.
        self._save_timer = wx.Timer(self)
        self.Bind(wx.EVT_TIMER, lambda evt: self._save(), self._save_timer)

        vbox = wx.BoxSizer(wx.VERTICAL)

        self.lbl_intro = wx.StaticText(self, label="")
        vbox.Add(self.lbl_intro, 0, wx.ALL, 8)

        # ---- Provider ----
        sb_prov, prov_box = _group(self, "Provider")
        prov_grid = wx.FlexGridSizer(1, 2, 6, 6)
        prov_grid.AddGrowableCol(1, 1)
        lbl_prov = wx.StaticText(sb_prov, label="Service:")
        self.choice_provider = wx.Choice(
            sb_prov, choices=[d for _p, d in self._providers]
        )
        self.choice_provider.SetToolTip(
            "Which AI service describes the scene. Each service keeps its own "
            "API key and model."
        )
        self.choice_provider.Bind(wx.EVT_CHOICE, self.on_change_provider)
        prov_grid.Add(lbl_prov, 0, wx.ALIGN_CENTER_VERTICAL)
        prov_grid.Add(self.choice_provider, 1, wx.EXPAND)
        prov_box.Add(prov_grid, 0, wx.EXPAND | wx.ALL, 6)
        vbox.Add(prov_box, 0, wx.EXPAND | wx.ALL, 6)

        # ---- API key ----
        # Kept on self so the group title can name the active provider.
        self._sb_key, key_box = _group(self, "API Key")
        # No SetName: the label flips between "Set API key" and "Clear API key"
        # at runtime, and a fixed name would go stale on every toggle.
        self.btn_api_key = wx.Button(self._sb_key, label="Set API key")
        self.btn_api_key.Bind(wx.EVT_BUTTON, self.on_api_key_button)
        key_box.Add(self.btn_api_key, 0, wx.ALL, 6)
        self.lbl_key_status = wx.StaticText(self._sb_key, label="")
        self.lbl_key_status.SetName("API key status")
        key_box.Add(self.lbl_key_status, 0, wx.LEFT | wx.BOTTOM, 6)
        vbox.Add(key_box, 0, wx.EXPAND | wx.ALL, 6)

        # ---- Model ----
        sb_model, model_box = _group(self, "Vision Model")
        grid = wx.FlexGridSizer(1, 2, 6, 6)
        grid.AddGrowableCol(1, 1)
        # The group box and this label together name the combo; a SetName on
        # top of them made readers announce it twice.
        lbl_model = wx.StaticText(sb_model, label="Model:")
        self.choice_model = wx.Choice(sb_model, choices=[d for _n, d in self._models])
        self.choice_model.Bind(wx.EVT_CHOICE, self.on_change)
        grid.Add(lbl_model, 0, wx.ALIGN_CENTER_VERTICAL)
        grid.Add(self.choice_model, 1, wx.EXPAND)
        model_box.Add(grid, 0, wx.EXPAND | wx.ALL, 6)
        vbox.Add(model_box, 0, wx.EXPAND | wx.ALL, 6)

        # ---- Provider-specific extras ----
        # Build a row per extra across every provider up front, then show only
        # the active provider's rows. Creating them once (rather than
        # destroying and rebuilding on each switch) keeps the tab order and the
        # screen reader's view of the panel stable.
        self._sb_extra, self._extra_box = _group(self, "Advanced")
        self._extra_rows = {}
        for pid, _disp in self._providers:
            for ex in self._ai.extras_for(pid):
                self._build_extra_row(ex)
        vbox.Add(self._extra_box, 0, wx.EXPAND | wx.ALL, 6)
        self._extra_outer = vbox.GetItem(self._extra_box)

        # ---- Capture options ----
        sb_opt, opt_box = _group(self, "Capture")
        self.chk_disable_ui_toggle = wx.CheckBox(
            sb_opt, label="Disable automatic UI hiding during capture"
        )
        self.chk_disable_ui_toggle.SetToolTip(
            "When unchecked, the game UI is briefly hidden while the screenshot is "
            "taken so HUD elements don't appear in the description. Check this to "
            "leave the UI visible (e.g. to have it described)."
        )
        self.chk_disable_ui_toggle.Bind(wx.EVT_CHECKBOX, self.on_change)
        opt_box.Add(self.chk_disable_ui_toggle, 0, wx.ALL, 6)
        vbox.Add(opt_box, 0, wx.EXPAND | wx.ALL, 6)

        self.SetSizer(vbox)

        self.load_into_controls(self.cur_cfg)

    # ---- Provider-specific extra controls ----

    def _build_extra_row(self, ex):
        """Create one labelled control for an extra-parameter descriptor.

        The row lives in its own horizontal sizer so `_set_row` can show/hide
        and enable/disable it as a unit when the provider changes.
        """
        row = wx.BoxSizer(wx.HORIZONTAL)
        lbl = wx.StaticText(self._sb_extra, label=ex["label"])
        if ex["kind"] == "choice":
            ctrl = wx.Choice(self._sb_extra, choices=[d for _v, d in ex["values"]])
            ctrl.Bind(wx.EVT_CHOICE, self.on_change)
        else:
            ctrl = wx.TextCtrl(self._sb_extra)
            # Saving per keystroke would rewrite the config file on every
            # character typed into the URL; debounce like the main tab does.
            ctrl.Bind(wx.EVT_TEXT, self._on_extra_text)
        ctrl.SetToolTip(ex["help"])
        # The group box and this label name the control between them; a SetName
        # on top would make readers announce it twice.
        row.Add(lbl, 0, wx.ALIGN_CENTER_VERTICAL | wx.RIGHT, 6)
        row.Add(ctrl, 1, wx.EXPAND)
        self._extra_box.Add(row, 0, wx.EXPAND | wx.ALL, 6)
        self._extra_rows[ex["key"]] = (row, ctrl, ex)

    def _on_extra_text(self, evt=None):
        self._save_timer.StartOnce(_AUTO_SAVE_DELAY_MS)
        if evt:
            evt.Skip()

    def flush_pending_save(self):
        """Write immediately if the debounce timer is still counting down."""
        if self._save_timer.IsRunning():
            self._save_timer.Stop()
            self._save()

    def _extra_value(self, ex, ctrl):
        """Read one extra control, falling back to its default when unusable."""
        if ex["kind"] == "choice":
            sel = ctrl.GetSelection()
            if 0 <= sel < len(ex["values"]):
                return ex["values"][sel][0]
            return ex["default"]
        return (ctrl.GetValue() or "").strip() or ex["default"]

    def _show_extras_for_provider(self):
        """Show the active provider's extra rows and hide everyone else's."""
        active = {ex["key"] for ex in self._ai.extras_for(self._provider)}
        for key, (row, ctrl, _ex) in self._extra_rows.items():
            # Hide *and* disable: hiding alone would leave the control out of
            # the tab order in an inconsistent state.
            _set_row(row, (ctrl,), key in active, self.choice_provider)
        # Drop the whole group when the provider declares no extras, so its
        # empty box doesn't sit there announcing itself.
        has_any = bool(active)
        self._extra_outer.Show(has_any)
        self._sb_extra.Show(has_any)

    # ---- Persistence ----

    def _save(self):
        """Merge this panel's keys onto a fresh disk read and write the file.

        Only the active provider's key/model/extras are written; the other
        providers' values come through untouched from the disk read.
        """
        try:
            cfg = load_config()
        except Exception:
            cfg = self.cur_cfg.copy()
        key_cfg, model_cfg = self._ai.config_keys_for(self._provider)
        cfg[self._ai.PROVIDER_CFG_KEY] = self._provider
        cfg[model_cfg] = self._selected_model_name()
        cfg[key_cfg] = self.cur_cfg.get(key_cfg, "")
        for ex in self._ai.extras_for(self._provider):
            _row, ctrl, _ex = self._extra_rows[ex["key"]]
            cfg[ex["key"]] = self._extra_value(ex, ctrl)
        cfg["ai_describer_disable_ui_toggle"] = self.chk_disable_ui_toggle.GetValue()
        try:
            _write_config(CONFIG_PATH, cfg)
            self.cur_cfg = cfg
        except Exception:
            pass

    def on_change(self, evt=None):
        self._save()
        if evt:
            evt.Skip()

    def on_change_provider(self, evt=None):
        """Repoint every provider-specific control at the newly chosen service."""
        # Flush any half-typed base URL against the *old* provider before the
        # key names change out from under it.
        self.flush_pending_save()
        self._provider = self._selected_provider()
        self._retitle_for_provider()
        self._load_provider_controls(self.cur_cfg)
        self._show_extras_for_provider()
        self._refresh_key_button()
        self._save()
        self.Layout()
        # ScrolledWindow: the panel just changed height, so the scroll extent
        # has to be recomputed or the last group can become unreachable.
        self.FitInside()
        if evt:
            evt.Skip()

    def _selected_provider(self):
        sel = self.choice_provider.GetSelection()
        if 0 <= sel < len(self._providers):
            return self._providers[sel][0]
        return self._ai.DEFAULT_PROVIDER

    def _selected_model_name(self):
        sel = self.choice_model.GetSelection()
        if 0 <= sel < len(self._models):
            return self._models[sel][0]
        return self._ai.default_model_for(self._provider)

    def _retitle_for_provider(self):
        """Put the provider's name on the labels that mention it."""
        display = self._ai.provider_display_name(self._provider)
        self._sb_key.SetLabel(f"{display} API Key")
        self.choice_model.SetToolTip(
            f"The {display} model used to describe the scene."
        )
        self.lbl_intro.SetLabel(
            "Press F10 then Space in-game to describe the current scene using "
            f"{display}. The description is spoken and added to the speech buffer."
        )
        self.lbl_intro.Wrap(560)

    def _load_provider_controls(self, cfg):
        """Fill the model dropdown and extras from the active provider's config."""
        self._models = list(self._ai.vision_models_for(self._provider))
        self.choice_model.Clear()
        self.choice_model.AppendItems([d for _n, d in self._models])
        _key_cfg, model_cfg = self._ai.config_keys_for(self._provider)
        want = cfg.get(model_cfg, self._ai.default_model_for(self._provider))
        idx = 0
        for i, (name, _disp) in enumerate(self._models):
            if name == want:
                idx = i
                break
        self.choice_model.SetSelection(idx)

        for ex in self._ai.extras_for(self._provider):
            _row, ctrl, _ex = self._extra_rows[ex["key"]]
            val = cfg.get(ex["key"], ex["default"])
            if ex["kind"] == "choice":
                values = [v for v, _d in ex["values"]]
                ctrl.SetSelection(values.index(val) if val in values else
                                  values.index(ex["default"]))
            else:
                ctrl.ChangeValue((val or "").strip() or ex["default"])

    def load_into_controls(self, cfg):
        self.cur_cfg = cfg
        provider = cfg.get(self._ai.PROVIDER_CFG_KEY, self._ai.DEFAULT_PROVIDER)
        if provider not in dict(self._providers):
            provider = self._ai.DEFAULT_PROVIDER
        self._provider = provider
        self.choice_provider.SetSelection(
            [p for p, _d in self._providers].index(provider)
        )
        self._retitle_for_provider()
        self._load_provider_controls(cfg)
        self._show_extras_for_provider()
        self.chk_disable_ui_toggle.SetValue(bool(cfg.get("ai_describer_disable_ui_toggle", False)))
        self._refresh_key_button()
        self.Layout()
        self.FitInside()

    def _has_key(self):
        key_cfg, _model_cfg = self._ai.config_keys_for(self._provider)
        return bool((self.cur_cfg.get(key_cfg, "") or "").strip())

    def _refresh_key_button(self):
        if self._has_key():
            self.btn_api_key.SetLabel("Clear API key")
            self.lbl_key_status.SetLabel("An API key is configured.")
        else:
            self.btn_api_key.SetLabel("Set API key")
            self.lbl_key_status.SetLabel("No API key configured.")
        self.Layout()

    # ---- API key button ----

    def on_api_key_button(self, evt):
        if self._has_key():
            self._clear_api_key()
        else:
            self._set_api_key()

    def _clear_api_key(self):
        display = self._ai.provider_display_name(self._provider)
        ans = wx.MessageBox(
            f"Remove your stored {display} API key?",
            "Clear API Key",
            wx.YES_NO | wx.NO_DEFAULT | wx.ICON_QUESTION,
            self,
        )
        if ans != wx.YES:
            return
        key_cfg, _model_cfg = self._ai.config_keys_for(self._provider)
        self.cur_cfg[key_cfg] = ""
        self._save()
        self._refresh_key_button()

    def _set_api_key(self):
        dlg = wx.TextEntryDialog(
            self,
            self._ai.provider_info(self._provider)["key_help"],
            "Set API Key",
        )
        try:
            if dlg.ShowModal() != wx.ID_OK:
                return
            key = (dlg.GetValue() or "").strip()
        finally:
            dlg.Destroy()
        if not key:
            return

        self.btn_api_key.Enable(False)
        self.lbl_key_status.SetLabel("Validating API key...")

        provider = self._provider
        # Validate against the endpoint the describer will actually call, so a
        # mistyped custom base URL or an out-of-reach model surfaces here rather
        # than in-game.
        extras = {
            ex["arg"]: self._extra_value(ex, self._extra_rows[ex["key"]][1])
            for ex in self._ai.extras_for(provider)
        }
        model = self._selected_model_name()

        def worker():
            ok, err = self._ai.validate_api_key(
                key, provider=provider, model=model, **extras
            )
            wx.CallAfter(self._on_validated, key, ok, err)

        import threading

        threading.Thread(target=worker, daemon=True).start()

    def _on_validated(self, key, ok, err):
        # The button was disabled while validation ran in the background, which
        # dropped focus off the very control the user had just pressed. Put it
        # back, so its new label ("Clear API key") is what gets announced.
        self.btn_api_key.Enable(True)
        self.btn_api_key.SetFocus()
        if ok:
            key_cfg, _model_cfg = self._ai.config_keys_for(self._provider)
            # The validated key goes to disk sealed, never in the clear. `key`
            # itself stays plaintext only for the validation call above, which
            # has already returned.
            self.cur_cfg[key_cfg] = secretstore.protect(key)
            self._save()
            self._refresh_key_button()
            wx.MessageBox(
                "API key validated and saved.",
                "API Key",
                wx.OK | wx.ICON_INFORMATION,
                self,
            )
        else:
            self._refresh_key_button()
            wx.MessageBox(
                f"The API key could not be validated:\n\n{err}",
                "Invalid API Key",
                wx.OK | wx.ICON_ERROR,
                self,
            )
