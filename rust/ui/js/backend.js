// The one place the frontend talks to the backend. Every command and event is listed in
// ../CONTRACT.md. With Tauri 2 (withGlobalTauri: true) calls go to the Rust side; opened
// straight from disk or a static server, an in-memory fake answers instead so the page can
// be driven with a screen reader before any Rust exists.

let impl;

const tauri = window.__TAURI__;
if (tauri && tauri.core && tauri.event) {
  impl = {
    isMock: false,
    invoke(command, args) {
      // Every command that takes arguments takes exactly one, named "args" (see
      // CONTRACT.md), so Tauri's camelCase renaming of parameter names never applies.
      return args === undefined
        ? tauri.core.invoke(command)
        : tauri.core.invoke(command, { args });
    },
    listen(event, handler) {
      return tauri.event.listen(event, (e) => handler(e.payload));
    },
  };
} else {
  const { createMockBackend } = await import("./mock-backend.js");
  impl = createMockBackend();
}

export const isMock = impl.isMock;

/** Call a backend command. Resolves with its return value; rejects with its error. */
export function invoke(command, args) {
  return impl.invoke(command, args);
}

/** Subscribe to a backend event. The handler receives the payload only. */
export function listen(event, handler) {
  return impl.listen(event, handler);
}

/**
 * Raise a native message box (Win32 MessageBoxW, owned by the main window) and wait for
 * the answer: "ok", "yes" or "no".
 *   kind:    "info" | "warning" | "error" | "question"
 *   buttons: "ok" | "yes_no"
 *   default: "ok" | "yes" | "no"
 */
export function messageBox(title, text, kind, buttons = "ok", defaultButton = "ok") {
  return invoke("message_box", {
    title,
    text,
    kind,
    buttons,
    default: defaultButton,
  });
}
