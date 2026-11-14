# Control inventory

Every wx control and static text in `BeamTelFrame` (beamtel.py), `ConfigPanel` and `AIDescriberPanel` (config_ui.py), the standalone configurator frame and the two web dialogs, against the HTML element that replaces it. Generated from a hand-written mapping and **checked by script**: every tooltip below was read from the Python source and compared character for character with the `title` in `index.html`, and every accessible name was compared with the name headless Chrome computes for the HTML element (Accessibility.getPartialAXTree). Order is source order, which is also the wx tab order and the HTML tab order.

Columns: **Name** is the wx accessible name (`SetName` / `_label_spin`) where it differs from the visible label, `-` for a static caption (not focusable). **Default** is `configurator.DEFAULT_CONFIG`. **Enable** / **Show**: `always` unless stated; wx hid and disabled a conditional row together (`_set_row`), and the HTML does the same with the `hidden` attribute on the row plus `disabled` on its controls. Where the tooltip column says *label*, the HTML puts it in `title` on the control's `<label>`; for buttons on a wrapping `span.tip`; for radio groups on the `<legend>`; for the two controls whose named label is visually hidden, on the visible caption wrapper.

| Source | wx class | Visible label | Name | Tooltip | Range / step / digits / choices | Config key | Default | Enable | Show | HTML id | HTML element |
|---|---|---|---|---|---|---|---|---|---|---|---|
| beamtel.py:11534 | Frame (title) | BeamNG Accessibility <APP_VERSION> | (label) | none | 700x700, min 600x500 | - | - | always | always | (window title, app_info.title) | native title + document.title |
| beamtel.py:11552 | Notebook | (tab strip) | BEAM Tabs | none | Main / Configuration / AI Describer | - | - | always | main mode | tablist | div role=tablist aria-label |
| beamtel.py:11623 | Notebook page | Main | (label) | none | - | - | - | always | selected tab | tab_main | button role=tab + section#panel_main role=tabpanel > h1 |
| beamtel.py:11624 | Notebook page | Configuration | (label) | none | - | - | - | always | selected tab | tab_config | button role=tab + section#panel_config role=tabpanel > h1 |
| beamtel.py:11625 | Notebook page | AI Describer | (label) | none | - | - | - | always | selected tab | tab_describer | button role=tab + section#panel_describer role=tabpanel > h1 |
| beamtel.py:11559 | TextCtrl (multiline, read-only, no wrap) | (none) | Application Log | none | - | - | - | always | always | log_text | textarea readonly wrap=off + visible label |
| beamtel.py:11571 | Button | View Logs... | (label) | Open one of the log files BEAM writes, or the folder holding them. | native popup menu | - | - | always | always | btn_view_logs | button (tooltip on span.tip) |
| beamtel.py:11855 | StaticBox | Developer Console | (label) | none | - | - | - | always | always | sb_console | fieldset > legend > h2 |
| beamtel.py:11860 | StaticText | Context: | - | none | - | - | - | always | always | label[for=console_ctx_choice] + text | label (caption) |
| beamtel.py:11861 | Choice | (label above) | Console Context | none | GE - Lua / GE - TorqueScript / CEF/UI - JS, then the mod's CTX list | - | - | always | always | console_ctx_choice | select |
| beamtel.py:11867 | Button | Refresh Contexts | (label) | none | - | - | - | always | always | console_btn_refresh | button |
| beamtel.py:11875 | StaticText | Command: | - | none | - | - | - | always | always | label[for=console_input] + text | label (caption) |
| beamtel.py:11876 | TextCtrl (TE_PROCESS_ENTER) | (label above) | Console Command | none | Enter runs; Up/Down history | - | - | always | always | console_input | input type=text |
| beamtel.py:11878 | Button | Run | (label) | none | - | - | - | always | always | console_btn_run | button |
| beamtel.py:11885 | TextCtrl (multiline, read-only, no wrap) | (none) | Console Output | none | - | - | - | always | always | console_output | textarea readonly wrap=off + visible label |
| beamtel.py:11894 | StaticText | Filter: | - | none | - | - | - | always | always | label[for=console_filter] + text | label (caption) |
| beamtel.py:11895 | TextCtrl | (label above) | Console Output Filter | none | substring, case-insensitive | - | - | always | always | console_filter | input type=text |
| beamtel.py:11897 | CheckBox | Stream game log | (label) | none | - | - (not saved) | - | always | always | console_log_check | input type=checkbox + label |
| beamtel.py:11898 | Button | Clear Output | (label) | none | - | - | - | always | always | console_btn_clear | button |
| beamtel.py:11585 | Button | Install Mod | (label) | Install the BeamNG.drive mod and activate its screen-reader UI app. | - | - | - | always | always | btn_install_mod | button (tooltip on span.tip) |
| beamtel.py:11590 | Button | Check for Updates | (label) | Ask GitHub whether a newer version of BEAM has been released. | - | - | - | always | always | btn_check_updates | button (tooltip on span.tip) |
| beamtel.py:11596 | Button | Exit | (label) | none | - | - | - | always | always | btn_exit | button |
| configurator.py:535 | Frame (title) | BeamTel Configurator | (label) | none | 600x940, min 580x940 | - | - | always | configurator mode | (window title, app_info.title) | native title + document.title; tab strip removed |
| config_ui.py:511 | StaticBox | Speech | (label) | none | - | - | - | always | always | sb_speech | fieldset > legend > h2 |
| config_ui.py:514 | StaticText | Speech Backend: | - | none | - | - | - | always | always | label[for=choice_backend] + text | label (caption) |
| config_ui.py:515 | Choice | (label above) | Speech Backend | Which screen reader or speech engine to talk to. Auto picks the best one that is running. | "Auto (best available)" + speech_list_backends() | speech_backend | "auto" | always | always | choice_backend | select |
| config_ui.py:523 | StaticText | Voice: | - | none | - | - | - | always | always | label[for=choice_voice] + text | label (caption) |
| config_ui.py:524 | Choice | (label above) | Voice | Voice to use, where the backend allows choosing one. Screen readers use their own voice settings. | speech_list_voices(backend).voices, or "Backend chooses its own voice" | speech_voice_name | "" | features.supports_set_voice (disabled when features is null) | always | choice_voice | select |
| config_ui.py:532 | StaticText | Rate (0 to 100): | - | none | - | - | - | always | always | label[for=spin_rate] + text | label (caption) |
| config_ui.py:533 | SpinCtrl | (label above) | Speech Rate | Speech rate: 0 (slow) to 100 (fast). | 0 to 100, inc 1 | speech_rate | 50 | features.supports_set_rate | always | spin_rate | input type=number |
| config_ui.py:539 | StaticText | Volume (0 to 100): | - | none | - | - | - | always | always | label[for=spin_volume] + text | label (caption) |
| config_ui.py:540 | SpinCtrl | (label above) | Speech Volume | Speech volume: 0 to 100. | 0 to 100, inc 1 | speech_volume | 100 | features.supports_set_volume | always | spin_volume | input type=number |
| config_ui.py:549 | StaticText (dynamic) | capabilities sentence | Backend Capabilities | none | "This backend supports: ..." / "This backend manages its own voice, rate, and volume." / "Backend not available on this machine." | - | - | always | always | lbl_speech_caps | p (not a live region) |
| config_ui.py:553 | Button | Refresh Voices | (label) | Re-detect speech backends and voices. | - | - | - | always | always | btn_refresh_voices | button (tooltip on span.tip) |
| config_ui.py:555 | Button | Test Voice | (label) | Speak a short test line with the selected voice, rate, and volume. | - | - | - | always | always | btn_test_voice | button (tooltip on span.tip) |
| config_ui.py:566 | StaticBox | General | (label) | none | - | - | - | always | always | sb_gen | fieldset > legend > h2 |
| config_ui.py:567 | RadioBox | Display Units | (label) | Choose how speeds, temps, and pressures are spoken. | Imperial (mph, °F, psi) / Metric (km/h, °C, bar) | units | "imperial" | always | always | rb_units | fieldset > legend > h3; radios rb_units_0, rb_units_1 |
| config_ui.py:576 | RadioBox | Telemetry Protocol | (label) | Must match the protocol selected in the BeamNG.drive UI app. | Extended / OutGauge | telemetry_protocol | "extended" | always | always | rb_proto | fieldset > legend > h3; radios rb_proto_0, rb_proto_1 |
| config_ui.py:587 | CheckBox | Launch BeamNG.drive on startup | (label) | Automatically launch BeamNG.drive via Steam when BeamTel starts. | - | launch_beamng | false | always | always | chk_launch_beamng | input type=checkbox + label |
| config_ui.py:597 | StaticText | Graphics API: | - | none | - | - | - | always | launch_beamng checked | label[for=choice_renderer] + text | label (caption) |
| config_ui.py:598 | Choice | (label above) | Graphics API | Which graphics API BeamNG.drive should use when launched automatically. | Direct3D 11 / Vulkan | beamng_renderer | "d3d" | launch_beamng checked | launch_beamng checked (row renderer_row) | choice_renderer | select |
| config_ui.py:607 | CheckBox | Check for updates on startup | (label) | Ask GitHub for the newest release when BeamTel starts, and offer to install it. Nothing is downloaded without your answer. | - | update_check_enabled | true | always | always | chk_update_check | input type=checkbox + label |
| config_ui.py:619 | StaticBox | Automatic announcements | (label) | none | - | - | - | always | always | sb_auto | fieldset > legend > h2 |
| config_ui.py:621 | CheckBox | Announce turn signals | (label) | Speak when the left, right, or hazard turn signals are activated or deactivated. | - | announce_turn_signals | true | always | always | chk_announce_turn_signals | input type=checkbox + label |
| config_ui.py:627 | CheckBox | Speed announcements | (label) | Automatically speak the current speed each time it crosses an interval threshold. | - | announce_speed | true | always | always | chk_announce_speed | input type=checkbox + label |
| config_ui.py:634 | StaticText | Announce every: | - | none | - | - | - | always | announce_speed checked | span.cap text (label is visually hidden) | plain text |
| config_ui.py:635 | Choice | (caption above) | Speed announcement interval | How many speed units must be crossed before a speed announcement is made. | Every 25 / 50 / 75 / 100 mph or km/h (follows Display Units) | speed_announce_interval | 25 | announce_speed checked | announce_speed checked (row speed_interval_row) | choice_speed_interval | select + visually hidden label |
| config_ui.py:644 | CheckBox | Speak drift scoring | (label) | With the game's drift display on in free roam, read its messages: tiers, bonuses, combo, crashes and the points scored, plus drift zone results. While the display shows, automatic speed and compass speech pause; compass sounds keep playing. | - | drift_speech_enabled | true | always | always | chk_drift_speech | input type=checkbox + label |
| config_ui.py:653 | StaticText | Report gear changes: | - | none | - | - | - | always | always | label[for=choice_gear_mode] + text | label (caption) |
| config_ui.py:654 | Choice | (label above) | Report gear changes | Speak the new gear each time the vehicle changes gears, play a short tone whose pitch says which gear it is, or report nothing. | Off / As speech / As audio | gear_report_mode | "speech" | always | always | choice_gear_mode | select |
| config_ui.py:668 | CheckBox | Report sport mode only once | (label) | Report sport mode once when you shift into it, as the word "Sport" or one brighter tone, and stay silent while the transmission steps through sport 1, sport 2 and so on, as a CVT does on its own. | - | gear_speech_sport_once | false | always | gear mode speech or audio (row gear_sport_row) | chk_gear_sport_once | input type=checkbox + label |
| config_ui.py:684 | StaticText | Forward gears: | - | none | - | - | - | always | gear mode speech | label[for=choice_gear_number_style] + text | label (caption) |
| config_ui.py:685 | Choice | (label above) | Forward gears | Whether forward gears are spoken as ordinals or as plain numbers. | Ordinals (first, second) / Numbers (1, 2) | gear_speech_number_style | "ordinal" | gear mode speech | gear mode speech (row gear_speech_row) | choice_gear_number_style | select |
| config_ui.py:702 | StaticText | Root note: | - | none | - | - | - | always | gear mode audio | label[for=choice_gear_root] + text | label (caption) |
| config_ui.py:703 | Choice | (label above) | Root note | The note neutral plays. Each forward gear steps one note up the scale from it; reverse is one note below and park two. | gear_report.ROOT_NOTES (12) | gear_tone_root | "C" | gear mode audio | gear mode audio (grid gear_audio_grid) | choice_gear_root | select |
| config_ui.py:711 | StaticText | Octave: | - | none | - | - | - | always | gear mode audio | label[for=choice_gear_octave] + text | label (caption) |
| config_ui.py:712 | Choice | (label above) | Octave | The octave of the root note. Octave 4 holds middle C. | 2 / 3 / 4 / 5 / 6 | gear_tone_octave | 4 | gear mode audio | gear mode audio (grid gear_audio_grid) | choice_gear_octave | select |
| config_ui.py:719 | StaticText | Scale: | - | none | - | - | - | always | gear mode audio | label[for=choice_gear_scale] + text | label (caption) |
| config_ui.py:720 | Choice | (label above) | Scale | Which notes the gears step through. The quarter-tone scales include notes between the keys of a piano. | gear_report.SCALES (16) | gear_tone_scale | "major" | gear mode audio | gear mode audio (grid gear_audio_grid) | choice_gear_scale | select |
| config_ui.py:728 | StaticText | Gear Tone Volume (dBFS): | - | none | - | - | - | always | gear mode audio (grid gear_audio_grid) | label[for=spin_gear_tone_level] + text | label (caption) |
| config_ui.py:729 | SpinCtrlDouble | (label above) | Gear Tone Volume | Peak level of the gear change tone. | -120 to 0, inc 1, 1 digit | gear_tone_level_dbfs | -3.0 | always | gear mode audio (grid gear_audio_grid) | spin_gear_tone_level | input type=number |
| config_ui.py:736 | Button | Preview gear tones | (label) | Play reverse, neutral, first to sixth gear, then drive and sport with these settings. Sport gears play brighter. | - | - | - | gear mode audio | gear mode audio (grid gear_audio_grid) | btn_gear_preview | button (tooltip on span.tip) |
| config_ui.py:755 | CheckBox | Announce clickspot actions | (label) | Speak success or failure feedback after activating a clickspot. | - | announce_clickspot_actions | false | always | always | chk_announce_clickspot_actions | input type=checkbox + label |
| config_ui.py:768 | CheckBox | Periodic scanner distance callouts | (label) | When the vehicle scanner is active and has a target, periodically announce the distance and direction to that target. | - | scanner_distance_callout_enabled | false | always | always | chk_scanner_callout | input type=checkbox + label |
| config_ui.py:776 | StaticText | Callout every: | - | none | - | - | - | always | scanner callouts checked | span.cap text (label is visually hidden) | plain text |
| config_ui.py:777 | Choice | (caption above) | Scanner callout interval | How often (in seconds) to announce the scanner target distance and direction. | 5 / 10 / 15 / 20 / 30 / 45 / 60 seconds | scanner_distance_callout_interval | 10 | scanner callouts checked | scanner callouts checked (row callout_interval_row) | choice_callout_interval | select + visually hidden label |
| config_ui.py:786 | CheckBox | Quiet menu navigation while holding a direction | (label) | When holding a direction to move quickly through a menu, list, or slider, speak the first item and then stay silent until you stop, announcing where you landed. | - | ui_nav_hold_suppression | true | always | always | chk_ui_nav_hold | input type=checkbox + label |
| config_ui.py:799 | StaticBox | Road guidance | (label) | none | - | - | - | always | always | sb_road | fieldset > legend > h2 |
| config_ui.py:800 | CheckBox | Lane correction guidance | (label) | Play directional correction pips, a centred straighten-now double pip, and a short recovery confirmation while on a road. | - | road_follow_guidance_enabled | true | always | always | chk_road_follow | input type=checkbox + label |
| config_ui.py:808 | CheckBox | Speak approaching intersections | (label) | Announce intersection shape and available exits about seven seconds ahead. | - | road_junction_speech_enabled | true | always | always | chk_road_junction_speech | input type=checkbox + label |
| config_ui.py:816 | CheckBox | Speak approaching stop signs | (label) | While road guidance or steering assistance is on, announce stop signs ahead and remind you within 12 meters. | - | road_stop_sign_speech_enabled | true | always | always | chk_road_stop_signs | input type=checkbox + label |
| config_ui.py:821 | CheckBox | Speak traffic lights and state changes | (label) | While road guidance or steering assistance is on, announce the light on your approach and changes while waiting. | - | road_traffic_light_speech_enabled | true | always | always | chk_road_traffic_lights | input type=checkbox + label |
| config_ui.py:827 | CheckBox | Speak map speed limit changes | (label) | Announce the current map speed limit and changes with road detection or steering assistance on. BeamNG may estimate these limits; they may differ from signs and are not safe turn speeds. F9, S still reads the limit when this option is off. | - | road_speed_limit_speech_enabled | true | always | always | chk_road_speed_limits | input type=checkbox + label |
| config_ui.py:835 | CheckBox | Speak road surface changes | (label) | Always on, with or without road detection: say the surface under the tires when it changes, such as dirt road, asphalt road or grass. No stock map names its streets, so the surface is what is announced. | - | road_surface_speech_enabled | true | always | always | chk_road_surface | input type=checkbox + label |
| config_ui.py:842 | CheckBox | Stop bar tone (manual driving and AI assist) | (label) | A tone that pulses faster towards the stop bar before a junction, stop point, gated road or dead end, holds steady while you are on the bar, and quietens once you stop there. Speech still announces stop signs and lights. Manual driving requires road guidance on. Uses Intersection Tone Volume. | - | road_junction_proximity_enabled | true | always | always | chk_road_junction_proximity | input type=checkbox + label |
| config_ui.py:853 | CheckBox | Intersection proximity and entry tones | (label) | Play an entry tone, plus a near-junction double pip when the stop bar tone is disabled. | - | road_junction_earcon_enabled | true | always | always | chk_road_junction_earcon | input type=checkbox + label |
| config_ui.py:861 | CheckBox | Include private roads | (label) | List private roads as exits from public roads in manual guidance: driveways, lots, yards and closed courses the map keeps traffic off. Most have no gate. Once you are on a private road its exits are always listed. | - | road_include_private | false | always | always | chk_road_private | input type=checkbox + label |
| config_ui.py:873 | StaticText | Off-road Beacon Volume (dBFS): | - | none | - | - | - | always | always | label[for=spin_road_beep] + text | label (caption) |
| config_ui.py:874 | SpinCtrlDouble | (label above) | Off-road Beacon Volume | Volume for the beacon that guides you back to a road. | -120 to 0, inc 1, 1 digit | road_beep_volume_db | -14.0 | always | always | spin_road_beep | input type=number |
| config_ui.py:886 | StaticText | Lane Correction Volume (dBFS): | - | none | - | - | - | always | always | label[for=spin_road_correction] + text | label (caption) |
| config_ui.py:889 | SpinCtrlDouble | (label above) | Lane Correction Volume | Volume for lane-correction, straighten-now, and recovery-confirmation cues. | -120 to 0, inc 1, 1 digit | road_correction_volume_db | -24.0 | always | always | spin_road_correction | input type=number |
| config_ui.py:901 | StaticText | Intersection Tone Volume (dBFS): | - | none | - | - | - | always | always | label[for=spin_road_junction] + text | label (caption) |
| config_ui.py:904 | SpinCtrlDouble | (label above) | Intersection Tone Volume | Volume for the near-intersection double pip and intersection-entry tone. | -120 to 0, inc 1, 1 digit | road_junction_volume_db | -14.0 | always | always | spin_road_junction | input type=number |
| config_ui.py:919 | StaticBox | Compass Clicks | (label) | none | - | - | - | always | always | sb_compass | fieldset > legend > h2 |
| config_ui.py:923 | StaticText | Click Interval (degrees): | - | none | - | - | - | always | always | label[for=spin_compass_interval] + text | label (caption) |
| config_ui.py:924 | SpinCtrl | (label above) | Compass Click Interval | The number of degrees of rotation before a compass click is heard (1-90). | 1 to 90, inc 1 | compass_click_interval | 15 | always | always | spin_compass_interval | input type=number |
| config_ui.py:933 | CheckBox | Highlight Every: | (label) | Play a distinct sound on a certain click. | - | compass_highlight_enabled | true | always | always | chk_compass_highlight | input type=checkbox + label |
| config_ui.py:938 | SpinCtrl | (checkbox before it) | Highlight every N clicks | Play the highlight sound on every Nth click (e.g., 4 for quadrants). | 2 to 100, inc 1 | compass_highlight_nth_click | 6 | highlight checked | highlight checked (row highlight_row) | spin_compass_highlight_nth | input type=number + visually hidden label |
| config_ui.py:952 | StaticText |  clicks | - | none | - | - | - | always | highlight checked (row highlight_row) | highlight_row > span | plain text |
| config_ui.py:959 | StaticText | Compass Click Volume (dBFS): | - | none | - | - | - | always | always | label[for=spin_compass_click_level] + text | label (caption) |
| config_ui.py:962 | SpinCtrlDouble | (label above) | Compass Click Volume | Volume level for compass clicks. 0 is loudest, negatives are quieter. | -120 to 0, inc 1, 1 digit | compass_click_level_dbfs | -6.0 | always | always | spin_compass_click_level | input type=number |
| config_ui.py:974 | StaticText | Low Speed Click Volume (dBFS): | - | none | - | - | - | always | always | label[for=spin_lowspeed_click_level] + text | label (caption) |
| config_ui.py:977 | SpinCtrlDouble | (label above) | Low Speed Click Volume | Volume level for low speed detection clicks. | -120 to 0, inc 1, 1 digit | lowspeed_click_level_dbfs | -14.0 | always | always | spin_lowspeed_click_level | input type=number |
| config_ui.py:989 | StaticText | Low Speed Stop Tone Volume (dBFS): | - | none | - | - | - | always | always | label[for=spin_lowspeed_stop_level] + text | label (caption) |
| config_ui.py:992 | SpinCtrlDouble | (label above) | Low Speed Stop Tone Volume | Volume level for the tone that confirms the vehicle has come to a stop. | -120 to 0, inc 1, 1 digit | lowspeed_stop_tone_level_dbfs | -16.0 | always | always | spin_lowspeed_stop_level | input type=number |
| config_ui.py:1004 | StaticText | Wheel Slip Tone Max Volume (dBFS): | - | none | - | - | - | always | always | label[for=spin_slip_tone_level] + text | label (caption) |
| config_ui.py:1007 | SpinCtrlDouble | (label above) | Wheel Slip Tone Max Volume | Loudest the wheel lockup, wheelspin and side-slide tones will play. They start quieter than this and reach it as the slip grows. | -120 to 0, inc 1, 1 digit | slip_tone_level_dbfs | -18.0 | always | always | spin_slip_tone_level | input type=number |
| config_ui.py:1020 | StaticText | Placement Ping Volume (dBFS): | - | none | - | - | - | always | always | label[for=spin_placement_ping_level] + text | label (caption) |
| config_ui.py:1023 | SpinCtrlDouble | (label above) | Placement Ping Volume | Volume level for the movement pings in the vehicle spawner's 3D placement editor. | -120 to 0, inc 1, 1 digit | placement_ping_volume_db | -12.0 | always | always | spin_placement_ping_level | input type=number |
| config_ui.py:1038 | CheckBox | HRTF Binaural Processing | (label) | Enable HRTF for 3D spatial audio on compass and low speed clicks. | - | hrtf_enabled | true | always | always | chk_hrtf_enabled | input type=checkbox + label |
| config_ui.py:1049 | StaticText | HRTF Dataset: | - | none | - | - | - | always | hrtf checked (grid hrtf_grid) | label[for=choice_hrtf_set] + text | label (caption) |
| config_ui.py:1050 | Choice | (label above) | HRTF Dataset | The head measurements used to place sounds around you. Each one sounds a little different; pick whichever makes directions clearest for you. Takes effect within a few seconds, no restart needed. | hrtf_sets.available_sets() (3) | hrtf_set | "mit_kemar" | hrtf checked | hrtf checked (grid hrtf_grid) | choice_hrtf_set | select |
| config_ui.py:1062 | StaticText | HRTF Elevation (degrees): | - | none | - | - | - | always | hrtf checked (grid hrtf_grid) | label[for=spin_hrtf_elevation] + text | label (caption) |
| config_ui.py:1063 | SpinCtrlDouble | (label above) | HRTF Elevation | How far below (negative) or above ear level directional sounds are placed. Small values mostly change the tone slightly; try around -10 or -15 to hear whether a lower placement suits you. | ELEVATION_MIN_DEG to ELEVATION_MAX_DEG (-20 to 10), inc 1, 0 digits | hrtf_elevation_deg | -3.0 | always | hrtf checked (grid hrtf_grid) | spin_hrtf_elevation | input type=number |
| config_ui.py:1080 | CheckBox | Equalise HRTF Tone | (label) | Removes the tone colouring that a dataset adds to every direction alike, such as the boxy or harsh sound of the measurement equipment, and keeps only the differences that tell directions apart. | - | hrtf_equalise | true | always | hrtf checked (grid hrtf_grid) | chk_hrtf_equalise | input type=checkbox + label |
| config_ui.py:1089 | StaticText | HRTF Front Emphasis (dB): | - | none | - | - | - | always | hrtf checked (grid hrtf_grid) | label[for=spin_hrtf_front_emphasis] + text | label (caption) |
| config_ui.py:1090 | SpinCtrlDouble | (label above) | HRTF Front Emphasis | Attenuation applied to sounds behind you. More negative = rear sounds quieter relative to front. | -24 to 0, inc 1, 1 digit | hrtf_front_emphasis_db | -6.0 | always | hrtf checked (grid hrtf_grid) | spin_hrtf_front_emphasis | input type=number |
| config_ui.py:1102 | StaticText | HRTF Distance Gain (dB): | - | none | - | - | - | always | hrtf checked (grid hrtf_grid) | label[for=spin_hrtf_distance_gain] + text | label (caption) |
| config_ui.py:1103 | SpinCtrlDouble | (label above) | HRTF Distance Gain | Additional gain after HRTF processing. Negative values make sounds appear farther away. | -24 to 6, inc 1, 1 digit | hrtf_distance_gain_db | 0.0 | always | hrtf checked (grid hrtf_grid) | spin_hrtf_distance_gain | input type=number |
| config_ui.py:1119 | StaticBox | Vehicle Scanner | (label) | none | - | - | - | always | always | sb_scanner | fieldset > legend > h2 |
| config_ui.py:1121 | CheckBox | Solid tone while steering | (label) | When the scanner is active and you are steering, morph the target beeps into a continuous directional tone so you can lock onto the target's direction even when the beeps are slow. Releasing the steering restores the normal beeps. | - | scanner_steer_tone_enabled | true | always | always | chk_scanner_steer_tone | input type=checkbox + label |
| config_ui.py:1134 | StaticText | Base Frequency (Hz): | - | none | - | - | - | always | always | label[for=spin_scanner_base_freq] + text | label (caption) |
| config_ui.py:1135 | SpinCtrl | (label above) | Scanner Base Frequency | Resting pitch of the scanner beeps/tone (when the target is directly behind). The pitch rises toward the target. | 100 to 8000, inc 1 | scanner_base_freq_hz | 1000.0 | always | always | spin_scanner_base_freq | input type=number |
| config_ui.py:1145 | StaticText | Alignment Pitch Offset (octaves): | - | none | - | - | - | always | always | label[for=spin_scanner_offset] + text | label (caption) |
| config_ui.py:1146 | SpinCtrlDouble | (label above) | Scanner Alignment Pitch Offset | How far the pitch rises (in octaves) when the target is dead-center, above the base frequency. Steps by one semitone; minimum half an octave, maximum two octaves. | 0.5 to 2.0, inc 1/12, 2 digits | scanner_pitch_offset_oct | 1.0 | always | always | spin_scanner_offset | input type=number |
| config_ui.py:1165 | StaticBox | Pitch & Roll | Pitch and Roll | none | - | - | - | always | always | sb_pitchroll | fieldset > legend > h2 |
| config_ui.py:1168 | CheckBox | Pitch and Roll Tones | (label) | Enable continuous tones that indicate vehicle pitch and roll angle. | - | pitch_roll_tones_enabled | true | always | always | chk_pitch_roll_enabled | input type=checkbox + label |
| config_ui.py:1177 | StaticText | Maximum Volume (dBFS): | - | none | - | - | - | always | always | label[for=spin_pitch_roll_max] + text | label (caption) |
| config_ui.py:1178 | SpinCtrlDouble | (label above) | Pitch Roll Maximum Volume | Volume level at maximum tilt angle. | -120 to 0, inc 1, 1 digit | pitch_roll_max_dbfs | -24.0 | always | always | spin_pitch_roll_max | input type=number |
| config_ui.py:1186 | StaticText | Stabilized Volume (dBFS): | - | none | - | - | - | always | always | label[for=spin_pitch_roll_min] + text | label (caption) |
| config_ui.py:1187 | SpinCtrlDouble | (label above) | Pitch Roll Stabilized Volume | Volume level when the vehicle is stable (faded down). | -120 to 0, inc 1, 1 digit | pitch_roll_min_dbfs | -36.0 | always | always | spin_pitch_roll_min | input type=number |
| config_ui.py:1203 | StaticBox | Shift Tone | (label) | none | - | - | - | always | always | sb_shift | fieldset > legend > h2 |
| config_ui.py:1206 | StaticText | Shift Tone Frequency (Hz): | - | none | - | - | - | always | always | label[for=spin_freq] + text | label (caption) |
| config_ui.py:1207 | SpinCtrlDouble | (label above) | Shift Tone Frequency | Triangle tone frequency when shift light is on. | 20 to 20000, inc 1, 1 digit | shift_tone_frequency_hz | 880.0 | always | always | spin_freq | input type=number |
| config_ui.py:1215 | StaticText | Shift Tone Level (dBFS): | - | none | - | - | - | always | always | label[for=spin_level] + text | label (caption) |
| config_ui.py:1216 | SpinCtrlDouble | (label above) | Shift Tone Level | Tone level; 0 is loudest, negatives are quieter. | -120 to 0, inc 1, 1 digit | shift_tone_level_dbfs | -12.0 | always | always | spin_level | input type=number |
| config_ui.py:1223 | Button | Test Shift Tone | (label) | Plays the shift tone using the current frequency and level settings. | - | - | - | AUDIO_TEST_OK (ui_choices.audio_test_ok) | always | btn_test_tone | button (tooltip on span.tip) |
| config_ui.py:1233 | StaticBox | Warning Sounds | (label) | none | - | - | - | always | always | sb_warnings | fieldset > legend > h2 |
| config_ui.py:1235 | CheckBox | Oil Overheating Alert | (label) | Enable the oil warning chime when the oil pressure warning light is on. | - | oil_chime_enabled | true | always | always | chk_oil_chime_enabled | input type=checkbox + label |
| config_ui.py:1241 | CheckBox | TC (Traction Control) Clicks | (label) | Enable the traction control activation click sound. | - | tc_clicks_enabled | true | always | always | chk_tc_clicks_enabled | input type=checkbox + label |
| config_ui.py:1254 | StaticText | Check Engine Buzzer Level (dBFS): | - | none | - | - | - | always | always | label[for=spin_buzzer_level] + text | label (caption) |
| config_ui.py:1255 | SpinCtrlDouble | (label above) | Check Engine Buzzer Level | Level for the check engine warning sound. | -120 to 0, inc 1, 1 digit | check_engine_buzzer_level_dbfs | -12.0 | always | always | spin_buzzer_level | input type=number |
| config_ui.py:1263 | StaticText | Oil Warning Chime Level (dBFS): | - | none | - | - | - | always | always | label[for=spin_chime_level] + text | label (caption) |
| config_ui.py:1264 | SpinCtrlDouble | (label above) | Oil Warning Chime Level | Level for the oil pressure warning chime. | -120 to 0, inc 1, 1 digit | oil_chime_level_dbfs | -12.0 | always | always | spin_chime_level | input type=number |
| config_ui.py:1272 | StaticText | Obstacle Cue Volume (dBFS): | - | none | - | - | - | always | always | label[for=spin_obstacle_level] + text | label (caption) |
| config_ui.py:1273 | SpinCtrlDouble | (label above) | Obstacle Cue Volume | Level of predictive static-obstacle cues; advisory cues play 6 dB lower. | -120 to 0, inc 1, 1 digit | obstacle_buzz_volume_db | -18.0 | always | always | spin_obstacle_level | input type=number |
| config_ui.py:1285 | StaticText | Obstacle Warning Sensitivity: | - | none | - | - | - | always | always | label[for=choice_obstacle_sensitivity] + text | label (caption) |
| config_ui.py:1288 | Choice | (label above) | Obstacle Warning Sensitivity | Changes advisory and urgent timing; emergency stopping distance is unchanged. | Early / Normal / Late | obstacle_warning_sensitivity | "normal" | always | always | choice_obstacle_sensitivity | select |
| config_ui.py:1304 | StaticBox | Loader Implement | (label) | none | - | - | - | always | always | sb_impl | fieldset > legend > h2 |
| config_ui.py:1306 | CheckBox | Bucket / Fork Tones | (label) | Ground proximity tone and tilt tone for a loader's bucket or forks. Both fade out when the controls are idle. | - | implement_tones_enabled | true | always | always | chk_impl_tones | input type=checkbox + label |
| config_ui.py:1315 | CheckBox | Announce Nearby Vehicles and Props | (label) | Speak the name of a vehicle or prop the bucket or forks are approaching, and whether they are above, below or level with it. | - | implement_proximity_speech | true | always | always | chk_impl_proximity | input type=checkbox + label |
| config_ui.py:1326 | CheckBox | Docking Instrument Tones | (label) | Alignment tones for lining the bucket or forks up with something: a pulse that pans left and right and speeds up as you close, and a pair of tones that beat against each other until the implement is level with the reference band. Only heard while the instrument is switched on in game with F9 then Ctrl+I. | - | dock_tones_enabled | true | always | always | chk_dock_tones | input type=checkbox + label |
| config_ui.py:1339 | StaticText | Ground Proximity Tone Level (dBFS): | - | none | - | - | - | always | always | label[for=spin_impl_ground] + text | label (caption) |
| config_ui.py:1342 | SpinCtrlDouble | (label above) | Ground Proximity Tone Level | Level of the tone that gets rougher as the bucket or forks near the ground. | -120 to 0, inc 1, 1 digit | implement_ground_tone_dbfs | -19.5 | always | always | spin_impl_ground | input type=number |
| config_ui.py:1352 | StaticText | Tilt Tone Level (dBFS): | - | none | - | - | - | always | always | label[for=spin_impl_tilt] + text | label (caption) |
| config_ui.py:1353 | SpinCtrlDouble | (label above) | Tilt Tone Level | Level of the quarter-tone tilt scale. 400 Hz is level, lower is tipped forward, higher is curled back. | -120 to 0, inc 1, 1 digit | implement_tilt_tone_dbfs | -20.0 | always | always | spin_impl_tilt | input type=number |
| config_ui.py:1364 | StaticText | Docking Tone Level (dBFS): | - | none | - | - | - | always | always | label[for=spin_dock] + text | label (caption) |
| config_ui.py:1365 | SpinCtrlDouble | (label above) | Docking Tone Level | Level of the docking pulse. The beating alignment pair is fixed relative to it, so that the two stay separable. | -120 to 0, inc 1, 1 digit | dock_tone_dbfs | -18.0 | always | always | spin_dock | input type=number |
| config_ui.py:1380 | StaticBox | Terrain Scanner | (label) | none | - | - | - | always | always | sb_scan | fieldset > legend > h2 |
| config_ui.py:1382 | CheckBox | Terrain Scan Tones | (label) | The terrain sonification scan, fired in game with F9 then Space while you are driving. Plays a reference tone and then a two second sweep of the ground ahead, where pitch is height, time is distance and the stereo position is the direction. Water is brighter, and vehicles and props are short bright pings. | - | scan_tones_enabled | true | always | always | chk_scan_tones | input type=checkbox + label |
| config_ui.py:1395 | StaticText | Terrain Scan Level (dBFS): | - | none | - | - | - | always | always | label[for=spin_scan] + text | label (caption) |
| config_ui.py:1396 | SpinCtrlDouble | (label above) | Terrain Scan Level | Level of the whole scan. Water and the vehicle pings sit at fixed offsets from it, so they stay in proportion at any setting. | -120 to 0, inc 1, 1 digit | scan_tone_dbfs | -20.0 | always | always | spin_scan | input type=number |
| config_ui.py:1411 | StaticBox | Large Cannon | (label) | none | - | - | - | always | always | sb_cannon | fieldset > legend > h2 |
| config_ui.py:1413 | CheckBox | Announce Shot Outcome | (label) | After a car is fired out of the large cannon, speak where it ended up once it comes to rest: how far downrange it went, how far off the firing line, and how much further or shorter than your previous shot. There is no key to press; the readout announces itself, because the moment the car stops is the only moment it has anything to say. | - | cannon_shot_readout | true | always | always | chk_cannon_shot | input type=checkbox + label |
| config_ui.py:1425 | StaticBox | Learn Bindings | (label) | none | - | - | - | always | always | sb_learn | fieldset > legend > h2 |
| config_ui.py:1427 | CheckBox | Speak Full Action Description | (label) | In learn bindings mode (F9 then Shift+B, or the Functions menu on a controller), every control you press is named along with what it does, and the binding does not fire -- so you can find the handbrake without the car spinning. By default each press is announced as just the control and the action, such as "A button. Shift up." Turn this on to also hear the game's full sentence of description for the action, which helps the first time through a pad and gets tiring once you know it. | - | binding_learn_speak_description | false | always | always | chk_learn_desc | input type=checkbox + label |
| config_ui.py:1441 | StaticBox | Audio Device | (label) | none | - | - | - | always | always | sb_audio | fieldset > legend > h2 |
| config_ui.py:1443 | CheckBox | Follow Default Audio Device | (label) | Automatically switch the audio output when the OS default device changes (recommended). | - | follow_default_audio_device | true | always | always | chk_follow_device | input type=checkbox + label |
| config_ui.py:1451 | StaticText | Fixed Output Device: | - | none | - | - | - | always | follow unchecked (row device_row) | label[for=choice_device] + text | label (caption) |
| config_ui.py:1452 | Choice | (label above) | Fixed Output Device | WASAPI output device to use when not following the OS default. | audio_list_devices(), or "(no WASAPI devices found)" | preferred_device_name | "" | follow unchecked | follow unchecked (row device_row) | choice_device | select |
| config_ui.py:1457 | Button | Refresh Devices | (label) | Re-scan for WASAPI output devices. | - | - | - | always (never disabled; hidden with the row) | follow unchecked (row device_row) | btn_refresh_device | button (tooltip on span.tip) |
| config_ui.py:1470 | StaticText | Device Poll Interval (sec): | - | none | - | - | - | always | always | label[for=spin_poll] + text | label (caption) |
| config_ui.py:1471 | SpinCtrlDouble | (label above) | Device Poll Interval | How often to check for default audio device changes (in seconds). | 0.1 to 10.0, inc 0.1, 1 digit | audio_poll_interval_sec | 2.0 | always | always | spin_poll | input type=number |
| config_ui.py:1485 | StaticBox | Developer | (label) | none | - | - | - | always | always | sb_dev | fieldset > legend > h2 |
| config_ui.py:1487 | CheckBox | Enable MCP automation server | (label) | Lets an AI assistant running on this computer drive the mod and execute code inside BeamNG.drive, so it can test features without you having to set each one up by hand. Listens on this machine only. Leave this off unless you are actively working with an assistant. Takes effect the next time BeamTel starts. | - | mcp_server_enabled | false | always | always | chk_mcp_server | input type=checkbox + label |
| config_ui.py:1501 | StaticText | MCP Server Port: | - | none | - | - | - | always | always | label[for=spin_mcp_port] + text | label (caption) |
| config_ui.py:1502 | SpinCtrl | (label above) | MCP Server Port | Which local port the automation server listens on. Change this only if another program already uses it. | 1024 to 65535, inc 1 | mcp_server_port | 4481 | always (no dependency on the checkbox in the Python) | always | spin_mcp_port | input type=number |
| config_ui.py:1517 | Button | Reset to Defaults | (label) | none | - | - | - | always | always | btn_reset | button |
| config_ui.py:2328 | StaticText (dynamic) | Press F10 then Space in-game ... using <display>. ... | (label) | none | - | - | - | always | always | lbl_intro | p (not a live region) |
| config_ui.py:2332 | StaticBox | Provider | (label) | none | - | - | - | always | always | sb_prov | fieldset > legend > h2 |
| config_ui.py:2335 | StaticText | Service: | - | none | - | - | - | always | always | label[for=choice_provider] | label |
| config_ui.py:2336 | Choice | (label above) | Service: | Which AI service describes the scene. Each service keeps its own API key and model. | Google Gemini / OpenAI (ai_describer.PROVIDERS) | ai_describer_provider | "gemini" | always | always | choice_provider | select |
| config_ui.py:2351 | StaticBox (retitled) | <display> API Key | (label) | none | - | - | - | always | always | sb_key | fieldset > legend > h2#sb_key_heading |
| config_ui.py:2354 | Button | Set API key / Clear API key | (its current label) | none | - | provider key_cfg (sealed) | - | disabled while validating | always | btn_api_key | button |
| config_ui.py:2357 | StaticText (dynamic) | No API key configured. / An API key is configured. / Validating API key... | API key status | none | - | - | - | always | always | lbl_key_status | p (not a live region) |
| config_ui.py:2363 | StaticBox | Vision Model | (label) | none | - | - | - | always | always | sb_model | fieldset > legend > h2 |
| config_ui.py:2368 | StaticText | Model: | - | none | - | - | - | always | always | lbl_model | label |
| config_ui.py:2369 | Choice | (label above) | Model: | (dynamic, see range column) | vision_models_for(provider); tooltip "The <display> model used to describe the scene." (2531) | provider model_cfg | - | always | always | choice_model | select |
| config_ui.py:2381 | StaticBox | Advanced | (label) | none | - | - | - | always | provider has extras | sb_extra | fieldset > legend > h2 |
| config_ui.py:2416 | StaticText | Base URL: | - | none | - | - | - | always | provider openai | label[for=extra_ai_describer_openai_base_url] | label |
| config_ui.py:2421 | TextCtrl | (label above) | Base URL: | registry `help` | registry extra (help text is the tooltip) | ai_describer_openai_base_url | "https://api.openai.com/v1" | provider openai | provider openai | extra_ai_describer_openai_base_url | input type=text |
| config_ui.py:2416 | StaticText | Reasoning effort: | - | none | - | - | - | always | provider openai | label[for=extra_ai_describer_openai_reasoning_effort] | label |
| config_ui.py:2418 | Choice | (label above) | Reasoning effort: | registry `help` | registry extra (help text is the tooltip) | ai_describer_openai_reasoning_effort | "low" | provider openai | provider openai | extra_ai_describer_openai_reasoning_effort | select |
| config_ui.py:2416 | StaticText | Image detail: | - | none | - | - | - | always | provider openai | label[for=extra_ai_describer_openai_detail] | label |
| config_ui.py:2418 | Choice | (label above) | Image detail: | registry `help` | registry extra (help text is the tooltip) | ai_describer_openai_detail | "auto" | provider openai | provider openai | extra_ai_describer_openai_detail | select |
| config_ui.py:2390 | StaticBox | Capture | (label) | none | - | - | - | always | always | sb_opt | fieldset > legend > h2 |
| config_ui.py:2391 | CheckBox | Disable automatic UI hiding during capture | (label) | When unchecked, the game UI is briefly hidden while the screenshot is taken so HUD elements don't appear in the description. Check this to leave the UI visible (e.g. to have it described). | - | ai_describer_disable_ui_toggle | false | always | always | chk_disable_ui_toggle | input type=checkbox + label |
| config_ui.py:2618 | TextEntryDialog | Set API Key | (label) | none | - | - | - | always | on Set API key | dlg_api_key | dialog (showModal) > form method=dialog > h2 |
| config_ui.py:2620 | TextEntryDialog message | <provider key_help> | (label) | none | - | - | - | always | in dialog | dlg_api_key_help | label for the text field |
| config_ui.py:2618 | TextEntryDialog text field | (message above) | <provider key_help> | none | - | - | - | always | in dialog | dlg_api_key_input | input type=text |
| config_ui.py:2618 | TextEntryDialog OK | OK | (label) | none | Enter | - | - | always | in dialog | dlg_api_key_ok | button type=submit value=ok |
| config_ui.py:2618 | TextEntryDialog Cancel | Cancel | (label) | none | Escape | - | - | always | in dialog | dlg_api_key_cancel | button type=submit value=cancel |
| updater.py:537 | ProgressDialog | <title> | (label) | none | - | - | - | always | update-progress events | dlg_update | dialog (showModal) > h2#dlg_update_title |
| updater.py:537 | ProgressDialog message | <message> [+ MB line] | (label) | none | - | - | - | always | in dialog | dlg_update_text | p (not a live region) |
| updater.py:537 | ProgressDialog gauge | (none) | <title> | none | 0 to 100 %, or Pulse | - | - | always | in dialog | dlg_update_progress | progress (indeterminate when no value) |
| updater.py:536 | ProgressDialog Cancel (PD_CAN_ABORT) | Cancel | (label) | none | - | - | - | always | cancellable (download only) | dlg_update_cancel | button |

## Native dialogs and menus (no HTML element)

| Python | Replacement |
|---|---|
| every `wx.MessageBox` raised by the panels (`on_test_voice`, `on_test_tone`, `on_preview_gear_tones`, `on_reset` x3, `_clear_api_key`, `_on_validated` x2, `_on_check_updates`) | `message_box` command -> Win32 `MessageBoxW` (CONTRACT.md lists title, text, icon, buttons and default for each) |
| every `wx.MessageBox` in `install_mod_interactive`, `updater.py`, `_open_file_if_exists`, `_on_open_log_folder` | raised natively by the backend inside `install_mod`, `check_updates`, the startup update flow and `logs_menu_popup` |
| `wx.Menu` popup from `_on_view_logs` (`beamtel.py:11764`) with the Application Log submenu, disabled "(not created yet)" items, separator, Open Log Folder | native popup menu built by the backend in `logs_menu_popup`, anchored under `#btn_view_logs` |

## Event handlers

| Python handler | Source | Where the behaviour lives |
|---|---|---|
| `wrap_nav_key` (Tab at a page's last control goes to the tab strip) | config_ui.py:428 | `js/tabs.js` `initTabs` document keydown; configurator mode (no notebook: wrap first <-> last) `initSingleWrap` |
| `BeamTelFrame._on_notebook_nav` (Tab from strip -> first control, Shift+Tab -> last) | beamtel.py:11816 | `js/tabs.js` `initTabs`: Tab from the selected tab reaches the first control natively (roving tabindex); Shift+Tab from the tab is handled to go to the last control |
| `wx.Notebook` arrow keys, Ctrl+Tab / Ctrl+Shift+Tab | (native) | `js/tabs.js` `initTabs` tablist keydown (Left/Right/Home/End) and document keydown (Ctrl+Tab) |
| `_focusable_leaves` | config_ui.py:416 | `js/tabs.js` `focusableLeaves` |
| `_owns_focus`, `_enable`, `_set_row` | config_ui.py:373-413 | `js/util.js` `ownsFocus`, `enable`, `setRow` |
| `_label_spin`, `LabelAccessible` | config_ui.py:335-357 | static HTML: `<label for>` text (with visually hidden words) equals the wx name |
| `_group` | config_ui.py:360 | static HTML: `fieldset > legend > h2` |
| `_bind_spin_double_page_keys` (PageUp/PageDown = 10 increments) | config_ui.py:463 | `js/util.js` `initSpin` keydown (also Up/Down = one increment, from the current value) |
| `ConfigPanel.on_toggle_highlight` | config_ui.py:1664 | `js/config-panel.js` `onToggleHighlight` |
| `on_toggle_hrtf` | 1676 | `onToggleHrtf` |
| `on_toggle_announce_speed` | 1691 | `onToggleAnnounceSpeed` |
| `on_change_gear_mode` | 1702 | `onChangeGearMode` |
| `on_preview_gear_tones` | 1734 | `onPreviewGearTones` -> `audio_preview_gear_tones` |
| `_update_speed_interval_labels`, `_on_units_changed` | 1745, 1754 | `updateSpeedIntervalLabels` (labels from `ui_choices.speed_intervals`), bound to `#rb_units` change |
| `_schedule_save` (2 s debounce, `_loading` guard) | 1761 | `scheduleSave` (`loading` flag) |
| `_auto_save` (merge AI keys and `pending_update_version` from disk) | 1771 | `autoSave` -> `config_save_panel`; the merge is in the backend (CONTRACT.md) |
| `ConfigPanel.flush_pending_save` | 1800 | `config-panel.js` `flushPendingSave`, called from `main.js` on `close-requested` |
| `load_into_controls` | 1813 | `loadIntoControls` (values normalised by `config_load` / `config_reset`) |
| `controls_to_config` | 1976 | `controlsToValues` (fallbacks for empty selections) + backend coercion and semitone snap |
| `_populate_backends`, `_select_backend`, `_selected_backend` | 2101-2124 | `populateBackends`, `selectBackend`, `selectedBackend` |
| `_apply_backend_capabilities` | 2126 | `applyBackendCapabilities` |
| `on_change_backend` | 2157 | `onChangeBackend` |
| `on_refresh_voices` (explicit refresh re-detects backends; selects the saved voice) | 2163 | `refreshVoices(explicit)` |
| `on_test_voice` | 2188 | `onTestVoice` -> `speech_test` |
| `on_test_tone` | 2199 | `onTestTone` -> `audio_test_tone` |
| `on_toggle_launch_beamng` | 2206 | `onToggleLaunchBeamng` |
| `on_toggle_scanner_callout` | 2218 | `onToggleScannerCallout` |
| `on_toggle_follow_device` | 2230 | `onToggleFollowDevice` |
| `on_refresh_devices` | 2244 | `refreshDevices` |
| `on_reset` (ask, reset, acknowledge) | 2257 | `onReset` -> `config_reset` |
| `AIDescriberPanel._build_extra_row` | 2409 | `js/describer-panel.js` `buildExtraRow` (rows from `ui_choices.ai_providers[*].extras`) |
| `_on_extra_text` (2 s debounce) | 2433 | `onExtraText` |
| `AIDescriberPanel.flush_pending_save` | 2438 | `describer-panel.js` `flushPendingSave` (close, and before a provider switch) |
| `_extra_value` | 2444 | `extraValue` |
| `_show_extras_for_provider` | 2453 | `showExtrasForProvider` |
| `_save` (merge onto fresh disk read) | 2468 | `save` -> `describer_save`; merge in backend |
| `on_change` | 2492 | `onChange` |
| `on_change_provider` (flush first) | 2497 | `onChangeProvider` |
| `_selected_provider`, `_selected_model_name` | 2515, 2521 | `selectedProvider`, `selectedModelName` |
| `_retitle_for_provider` | 2527 | `retitleForProvider` |
| `_load_provider_controls` | 2540 | `loadProviderControls` |
| `load_into_controls` | 2564 | `loadIntoControls` (from `describer_load`) |
| `_has_key`, `_refresh_key_button` | 2581, 2585 | `hasKey` (`state.providers[p].has_key`), `refreshKeyButton` |
| `on_api_key_button`, `_clear_api_key` | 2596, 2602 | `onApiKeyButton`, `clearApiKey` -> `describer_save` with `key_action: "clear"` |
| `_set_api_key` (TextEntryDialog, background validation, button disabled) | 2617 | `setApiKey` -> `js/dialogs.js` `askApiKey`, then `describer_validate_key` |
| `_on_validated` (re-enable, refocus, save sealed, message) | 2655 | `onValidated` -> `describer_save` with `key_action: "set"` |
| `WxLogHandler.emit/_append` | beamtel.py:11489 | `log-line` event -> `js/main-tab.js` `appendLog` (`setRangeText`, caret preserved) |
| `_on_check_updates` | 11650 | `onCheckUpdates` -> `check_updates` |
| `_open_file_if_exists`, `_log_menu_entries`, `_on_view_logs`, `_on_open_log_folder` | 11674-11812 | backend (`logs_menu_popup`); frontend `onViewLogs` sends the anchor |
| `_build_console_section` | 11846 | static HTML `#sb_console` + `initMainTab` |
| `_console_matches_filter`, `_console_append`, `_console_rerender` | 11918-11937 | `matchesFilter`, `consoleAppend`, `consoleRerender` |
| `_on_console_filter_changed` | 11939 | `#console_filter` input -> `consoleRerender` |
| `_on_console_clear` | 11942 | `consoleClear` |
| `_on_console_run` | 11946 | `consoleRun` (echo, local history) -> `console_run` (persist, send EXEC) |
| `_on_console_input_key` (Up/Down prefix-search history) | 11973 | `onInputKey` (also Enter = run, `TE_PROCESS_ENTER`) |
| `_on_console_log_toggle` | 12023 | `#console_log_check` change -> `console_set_log_stream` |
| Refresh Contexts lambda (`CTXLIST`) | 11909 | `console_refresh_contexts` |
| `_rebuild_context_choice` | 12026 | `console-contexts` event -> `onContexts` / `setContexts` (keeps the selection by index) |
| `on_console_message` (CTX, CTXEND, RESP, OUT, EXECEND, LOG, speaking a one-line result) | 12044 | backend; emits `console-line` / `console-contexts` |
| `_on_close` (flush both panels, stop, join) | 12107 | `main.js` `close-requested` listener -> `close_ready`; rest in the backend |
| Exit button `self.Close()` | 11645 | `exit_app` |
| Install Mod lambda | 11643 | `install_mod` |
| `configurator.main._on_close` (flush) | configurator.py:539 | `main.js` configurator branch of the same listener |
| `updater._run_with_progress` dialog loop and cancel | updater.py:513 | `update-progress` / `update-progress-close` events -> `js/dialogs.js` `onUpdateProgress`; Cancel / Escape -> `update_cancel` |
