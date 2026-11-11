export type ThemePref = "dark" | "light" | "system";

const KEY = "envdesk-theme";

export function loadThemePref(): ThemePref {
  const v = localStorage.getItem(KEY);
  if (v === "dark" || v === "light" || v === "system") return v;
  return "dark";
}

export function saveThemePref(pref: ThemePref) {
  localStorage.setItem(KEY, pref);
}

export function resolvedTheme(pref: ThemePref): "dark" | "light" {
  if (pref === "system") {
    return window.matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark";
  }
  return pref;
}

export function applyTheme(pref: ThemePref) {
  saveThemePref(pref);
  document.documentElement.dataset.theme = resolvedTheme(pref);
}

export function watchSystemTheme(pref: ThemePref, onChange: () => void) {
  if (pref !== "system") return () => undefined;
  const mq = window.matchMedia("(prefers-color-scheme: light)");
  const fn = () => onChange();
  mq.addEventListener("change", fn);
  return () => mq.removeEventListener("change", fn);
}
