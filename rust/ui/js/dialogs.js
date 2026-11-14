// The two web dialogs. Everything else that wx showed as a dialog is a native Win32
// message box raised by the backend (backend.messageBox), as wx.MessageBox was.

import { $, } from "./util.js";
import { invoke, listen } from "./backend.js";

// ---------------------------------------------------------------------------
// wx.TextEntryDialog(parent, key_help, "Set API Key")
// ---------------------------------------------------------------------------

/**
 * Show the API key entry dialog. Resolves with the entered text when OK is chosen (Enter
 * included), or null on Cancel / Escape. Focus returns to whatever opened it, which the
 * browser restores when a modal <dialog> closes.
 */
export function askApiKey(helpText) {
  const dlg = $("dlg_api_key");
  const form = $("dlg_api_key_form");
  const input = $("dlg_api_key_input");
  $("dlg_api_key_help").textContent = helpText;
  input.value = "";
  return new Promise((resolve) => {
    let result = null;
    const onSubmit = (e) => {
      result = e.submitter && e.submitter.value === "ok" ? input.value : null;
    };
    const onClose = () => {
      form.removeEventListener("submit", onSubmit);
      dlg.removeEventListener("close", onClose);
      input.value = "";
      resolve(result);
    };
    form.addEventListener("submit", onSubmit);
    dlg.addEventListener("close", onClose);
    dlg.showModal();
    input.focus();
  });
}

// Enter in the text field submits through the form's first submit button (OK), which is
// how TextEntryDialog's default button behaved. Escape closes with no submitter (Cancel).

// ---------------------------------------------------------------------------
// Update progress (updater._run_with_progress: wx.ProgressDialog)
// ---------------------------------------------------------------------------

let updateCancellable = false;

function onUpdateProgress(p) {
  const dlg = $("dlg_update");
  const bar = $("dlg_update_progress");
  const cancel = $("dlg_update_cancel");
  $("dlg_update_title").textContent = p.title;
  $("dlg_update_progress_label").textContent = p.title;
  $("dlg_update_text").textContent = p.text;
  if (p.fraction === null || p.fraction === undefined) {
    // Indeterminate, like ProgressDialog.Pulse().
    bar.removeAttribute("value");
  } else {
    bar.value = Math.min(1, Math.max(0, p.fraction));
  }
  updateCancellable = !!p.cancellable;
  cancel.hidden = !updateCancellable;
  cancel.disabled = false;
  if (!dlg.open) {
    dlg.showModal();
    if (updateCancellable) cancel.focus();
    else dlg.focus();
  }
}

function onUpdateProgressClose() {
  const dlg = $("dlg_update");
  if (dlg.open) dlg.close();
}

function requestCancel() {
  if (!updateCancellable) return;
  // The dialog stays up until the backend's worker sees the flag and the flow sends
  // update-progress-close, as wx's dialog stayed up until its loop ended.
  // Focus moves to the dialog before the button is disabled, so it is not stranded.
  const cancel = $("dlg_update_cancel");
  if (document.activeElement === cancel) $("dlg_update").focus();
  cancel.disabled = true;
  invoke("update_cancel").catch((err) => console.error("update_cancel failed", err));
}

export async function initUpdateDialog() {
  const dlg = $("dlg_update");
  $("dlg_update_cancel").addEventListener("click", requestCancel);
  // Escape must not close the dialog out from under the flow. On a cancellable dialog it
  // means Cancel; otherwise it does nothing.
  dlg.addEventListener("cancel", (e) => {
    e.preventDefault();
    requestCancel();
  });
  await listen("update-progress", onUpdateProgress);
  await listen("update-progress-close", onUpdateProgressClose);
}
