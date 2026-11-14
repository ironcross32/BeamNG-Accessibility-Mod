// The tab strip (wx.Notebook "BEAM Tabs") and the Tab-key wrap that BeamTelFrame and
// config_ui.wrap_nav_key gave every page:
//
//     ... -> last control -> tab strip -> first control -> ...
//
// Left/Right/Home/End on the strip select a tab (wx.Notebook selects on arrows). Ctrl+Tab
// and Ctrl+Shift+Tab switch tabs from anywhere in the window, as wx.Notebook does; focus
// then lands on the newly selected tab so its name is announced.

const FOCUSABLE = "button, input, select, textarea, a[href], [tabindex]";

/** Keyboard-focusable controls inside `root`, in tab order (_focusable_leaves). */
export function focusableLeaves(root) {
  return [...root.querySelectorAll(FOCUSABLE)].filter((el) => {
    if (el.disabled) return false;
    if (el.tabIndex < 0) return false;
    if (el.closest("[hidden]")) return false;
    // Only the checked radio of a group is a tab stop.
    if (el.type === "radio" && !el.checked) {
      const group = root.querySelectorAll(`input[type="radio"][name="${el.name}"]`);
      if ([...group].some((r) => r.checked)) return false;
    }
    return el.checkVisibility ? el.checkVisibility() : true;
  });
}

/**
 * Tabbed mode. `onSelect(panelId)` is called after a tab becomes selected.
 */
export function initTabs(tablist, onSelect) {
  const tabs = [...tablist.querySelectorAll('[role="tab"]')];
  const panelOf = (tab) => document.getElementById(tab.getAttribute("aria-controls"));
  const current = () => tabs.find((t) => t.getAttribute("aria-selected") === "true") || tabs[0];

  function select(tab, focus) {
    for (const t of tabs) {
      const on = t === tab;
      t.setAttribute("aria-selected", on ? "true" : "false");
      t.tabIndex = on ? 0 : -1;
      panelOf(t).hidden = !on;
    }
    if (focus) tab.focus();
    onSelect(panelOf(tab).id);
  }

  tablist.addEventListener("click", (e) => {
    const tab = e.target.closest('[role="tab"]');
    if (tab) select(tab, true);
  });

  tablist.addEventListener("keydown", (e) => {
    if (e.ctrlKey || e.altKey || e.metaKey) return;
    const i = tabs.indexOf(current());
    let next = null;
    if (e.key === "ArrowRight") next = tabs[(i + 1) % tabs.length];
    else if (e.key === "ArrowLeft") next = tabs[(i - 1 + tabs.length) % tabs.length];
    else if (e.key === "Home") next = tabs[0];
    else if (e.key === "End") next = tabs[tabs.length - 1];
    if (!next) return;
    e.preventDefault();
    select(next, true);
  });

  document.addEventListener("keydown", (e) => {
    if (e.key !== "Tab" || e.altKey || e.metaKey) return;
    if (document.querySelector("dialog[open]")) return;

    // Ctrl+Tab / Ctrl+Shift+Tab: next / previous page, from anywhere.
    if (e.ctrlKey) {
      e.preventDefault();
      const i = tabs.indexOf(current());
      const step = e.shiftKey ? -1 : 1;
      select(tabs[(i + step + tabs.length) % tabs.length], true);
      return;
    }

    const tab = current();
    const leaves = focusableLeaves(panelOf(tab));
    if (!leaves.length) return;
    const active = document.activeElement;
    if (!e.shiftKey && active === leaves[leaves.length - 1]) {
      // wrap_nav_key: Tab from the last control goes to the tab strip.
      e.preventDefault();
      tab.focus();
    } else if (e.shiftKey && active === tab) {
      // _on_notebook_nav: Shift+Tab from the strip goes to the page's last control.
      e.preventDefault();
      leaves[leaves.length - 1].focus();
    }
  });

  select(current(), false);
  return { select: (panelId) => select(tabs.find((t) => panelOf(t).id === panelId), false) };
}

/**
 * Configurator mode: no notebook, so wrap_nav_key's fallback applies and Tab wraps
 * directly between the panel's last and first controls.
 */
export function initSingleWrap(panel) {
  document.addEventListener("keydown", (e) => {
    if (e.key !== "Tab" || e.ctrlKey || e.altKey || e.metaKey) return;
    if (document.querySelector("dialog[open]")) return;
    const leaves = focusableLeaves(panel);
    if (!leaves.length) return;
    const active = document.activeElement;
    if (!e.shiftKey && active === leaves[leaves.length - 1]) {
      e.preventDefault();
      leaves[0].focus();
    } else if (e.shiftKey && active === leaves[0]) {
      e.preventDefault();
      leaves[leaves.length - 1].focus();
    }
  });
}
