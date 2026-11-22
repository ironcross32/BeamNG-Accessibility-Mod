// Boot. Two modes, chosen by app_info().mode:
//   "main"          the three-tab window (BeamTelFrame)
//   "configurator"  the Configuration panel alone (configurator.main), no tab strip

import { invoke, listen } from "./backend.js";
import { $ } from "./util.js";
import { initTabs, initSingleWrap, focusableLeaves } from "./tabs.js";
import { initUpdateDialog } from "./dialogs.js";
import { initMainTab, applyBacklog } from "./main-tab.js";
import * as configPanel from "./config-panel.js";
import * as describerPanel from "./describer-panel.js";

async function boot() {
  const info = await invoke("app_info");
  document.title = info.title;
  const choices = await invoke("ui_choices");
  const configurator = info.mode === "configurator";

  let closing = false;
  // _on_close: commit the debounced saves of both panels, then let the backend stop
  // the engine and destroy the window.
  await listen("close-requested", async () => {
    if (closing) return;
    closing = true;
    const panels = configurator ? [configPanel] : [configPanel, describerPanel];
    for (const panel of panels) {
      try {
        await panel.flushPendingSave();
      } catch (err) {
        console.error("flush_pending_save failed", err);
      }
    }
    await invoke("close_ready");
  });

  if (configurator) {
    // The document is the Configuration panel alone under its <h1>; the tab pattern
    // and the other panels are removed rather than hidden.
    $("tablist").remove();
    $("panel_main").remove();
    $("panel_describer").remove();
    $("dlg_api_key").remove();
    const panel = $("panel_config");
    panel.removeAttribute("role");
    panel.removeAttribute("aria-labelledby");
    panel.hidden = false;
    await initUpdateDialog();
    await configPanel.initConfigPanel(choices);
    initSingleWrap(panel);
    // Initial focus before frontend_ready, so a dialog the backend raises afterwards
    // returns focus here when it closes.
    const first = focusableLeaves(panel)[0];
    if (first) first.focus();
    await invoke("frontend_ready");
    return;
  }

  const tabs = initTabs($("tablist"), () => {});
  await initUpdateDialog();
  const history = await invoke("console_history");
  await initMainTab(choices, history);
  await configPanel.initConfigPanel(choices);
  await describerPanel.initDescriberPanel(choices);
  tabs.select("panel_main");
  $("tab_main").focus();
  // Listeners are registered; the backend now hands over what it buffered and may start
  // the startup update flow.
  applyBacklog(await invoke("frontend_ready"));
}

boot().catch((err) => console.error("BEAM interface failed to start", err));
