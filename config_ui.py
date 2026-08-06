# config_ui.py
# UI components for BeamTel configuration.
# Extracted from configurator.py so the panel can be embedded in the unified app.

import datetime
import json
import os
import shutil
import tempfile
import wx
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


def install_mod_interactive(parent):
    """Run the mod installation flow, showing wx dialogs as needed. Call from any wx context."""
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
        return

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
        return

    # Step 4: Decide whether to copy.
    dst_zip = os.path.join(mods_dir, _MOD_ZIP)
    do_copy = False
    mod_result = "kept the existing installed file"
    if not os.path.isfile(dst_zip):
        do_copy = True
    else:
        src_mtime = os.path.getmtime(src_zip)
        dst_mtime = os.path.getmtime(dst_zip)
        if src_mtime < dst_mtime:
            ans = wx.MessageBox(
                "The mod file in the program directory is older than the one already installed.\n"
                "Do you want to copy it anyway, overwriting the newer version?",
                "Older File Detected",
                wx.YES_NO | wx.NO_DEFAULT | wx.ICON_WARNING,
                parent,
            )
            if ans == wx.YES:
                do_copy = True
        elif src_mtime > dst_mtime:
            do_copy = True
        else:
            ans = wx.MessageBox(
                "The mod file already installed appears to be the same version as the one in the program directory.\n"
                "Do you want to copy it over anyway?",
                "Same Version Detected",
                wx.YES_NO | wx.NO_DEFAULT | wx.ICON_QUESTION,
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
            return

        # Step 5: Verify the result.
        if not os.path.isfile(dst_zip):
            wx.MessageBox(
                "The copy appeared to succeed but the destination file cannot be found.\n"
                "Installation may have failed.",
                "Verification Failed",
                wx.OK | wx.ICON_WARNING,
                parent,
            )
            return
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
        ):
            ctrl.Bind(wx.EVT_SPINCTRL, self._schedule_save)

        for ctrl in (
            self.spin_freq,
            self.spin_level,
            self.spin_buzzer_level,
            self.spin_chime_level,
            self.spin_compass_click_level,
            self.spin_lowspeed_click_level,
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
                disk = load_config()
                for k in (
                    "ai_describer_api_key",
                    "ai_describer_model",
                    "ai_describer_disable_ui_toggle",
                ):
                    if k in disk:
                        cfg[k] = disk[k]
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

        cfg["compass_click_interval"] = self.spin_compass_interval.GetValue()
        cfg["compass_highlight_enabled"] = self.chk_compass_highlight.GetValue()
        cfg["compass_highlight_nth_click"] = self.spin_compass_highlight_nth.GetValue()
        cfg["compass_click_level_dbfs"] = self.spin_compass_click_level.GetValue()
        cfg["lowspeed_click_level_dbfs"] = self.spin_lowspeed_click_level.GetValue()
        cfg["hrtf_enabled"] = self.chk_hrtf_enabled.GetValue()
        cfg["hrtf_front_emphasis_db"] = self.spin_hrtf_front_emphasis.GetValue()
        cfg["hrtf_distance_gain_db"] = self.spin_hrtf_distance_gain.GetValue()
        cfg["pitch_roll_tones_enabled"] = self.chk_pitch_roll_enabled.GetValue()
        cfg["pitch_roll_max_dbfs"] = self.spin_pitch_roll_max.GetValue()
        cfg["pitch_roll_min_dbfs"] = self.spin_pitch_roll_min.GetValue()
        cfg["oil_chime_enabled"] = self.chk_oil_chime_enabled.GetValue()
        cfg["tc_clicks_enabled"] = self.chk_tc_clicks_enabled.GetValue()
        cfg["launch_beamng"] = self.chk_launch_beamng.GetValue()
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
    """Configuration for the AI Describer feature (Gemini scene description).

    Owns three config keys: ai_describer_api_key, ai_describer_model and
    ai_describer_disable_ui_toggle. Writes are merged onto a fresh disk read so
    this panel doesn't clobber settings owned by the main Configuration tab.
    """

    _AI_KEYS = (
        "ai_describer_api_key",
        "ai_describer_model",
        "ai_describer_disable_ui_toggle",
    )

    def __init__(self, parent):
        super().__init__(parent, style=wx.TAB_TRAVERSAL)
        self.SetScrollRate(0, 10)
        self.cur_cfg = load_config()

        self.Bind(wx.EVT_NAVIGATION_KEY, lambda evt: wrap_nav_key(evt, self))

        from ai_describer import VISION_MODELS, DEFAULT_MODEL

        self._models = list(VISION_MODELS)
        self._default_model = DEFAULT_MODEL

        vbox = wx.BoxSizer(wx.VERTICAL)

        intro = wx.StaticText(
            self,
            label=(
                "Press F10 then Space in-game to describe the current scene using "
                "Google Gemini. The description is spoken and added to the speech "
                "buffer."
            ),
        )
        intro.Wrap(560)
        vbox.Add(intro, 0, wx.ALL, 8)

        # ---- API key ----
        sb_key, key_box = _group(self, "Gemini API Key")
        # No SetName: the label flips between "Set API key" and "Clear API key"
        # at runtime, and a fixed name would go stale on every toggle.
        self.btn_api_key = wx.Button(sb_key, label="Set API key")
        self.btn_api_key.Bind(wx.EVT_BUTTON, self.on_api_key_button)
        key_box.Add(self.btn_api_key, 0, wx.ALL, 6)
        self.lbl_key_status = wx.StaticText(sb_key, label="")
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
        self.choice_model.SetToolTip("The Gemini model used to describe the scene.")
        self.choice_model.Bind(wx.EVT_CHOICE, self.on_change)
        grid.Add(lbl_model, 0, wx.ALIGN_CENTER_VERTICAL)
        grid.Add(self.choice_model, 1, wx.EXPAND)
        model_box.Add(grid, 0, wx.EXPAND | wx.ALL, 6)
        vbox.Add(model_box, 0, wx.EXPAND | wx.ALL, 6)

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

    # ---- Persistence ----

    def _save(self):
        """Merge this panel's keys onto a fresh disk read and write the file."""
        try:
            cfg = load_config()
        except Exception:
            cfg = self.cur_cfg.copy()
        cfg["ai_describer_model"] = self._selected_model_name()
        cfg["ai_describer_disable_ui_toggle"] = self.chk_disable_ui_toggle.GetValue()
        cfg["ai_describer_api_key"] = self.cur_cfg.get("ai_describer_api_key", "")
        try:
            _write_config(CONFIG_PATH, cfg)
            self.cur_cfg = cfg
        except Exception:
            pass

    def on_change(self, evt=None):
        self._save()
        if evt:
            evt.Skip()

    def _selected_model_name(self):
        sel = self.choice_model.GetSelection()
        if 0 <= sel < len(self._models):
            return self._models[sel][0]
        return self._default_model

    def load_into_controls(self, cfg):
        self.cur_cfg = cfg
        want = cfg.get("ai_describer_model", self._default_model)
        idx = 0
        for i, (name, _disp) in enumerate(self._models):
            if name == want:
                idx = i
                break
        self.choice_model.SetSelection(idx)
        self.chk_disable_ui_toggle.SetValue(bool(cfg.get("ai_describer_disable_ui_toggle", False)))
        self._refresh_key_button()

    def _has_key(self):
        return bool((self.cur_cfg.get("ai_describer_api_key", "") or "").strip())

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
        ans = wx.MessageBox(
            "Remove your stored Gemini API key?",
            "Clear API Key",
            wx.YES_NO | wx.NO_DEFAULT | wx.ICON_QUESTION,
            self,
        )
        if ans != wx.YES:
            return
        self.cur_cfg["ai_describer_api_key"] = ""
        self._save()
        self._refresh_key_button()

    def _set_api_key(self):
        dlg = wx.TextEntryDialog(
            self,
            "Paste your Google Gemini API key:",
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

        def worker():
            from ai_describer import validate_api_key

            ok, err = validate_api_key(key)
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
            self.cur_cfg["ai_describer_api_key"] = key
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
