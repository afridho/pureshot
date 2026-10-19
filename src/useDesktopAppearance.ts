import { useEffect, useState } from "react";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow, type Theme } from "@tauri-apps/api/window";

export const nativeLinuxTitlebar = isTauri() && /Linux/.test(navigator.userAgent);

/** One OS preference for every WebView, with media-query/browser fallback. */
export function useDesktopAppearance() {
  const [theme, setTheme] = useState<Theme>(() => matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light");
  useEffect(() => {
    let disposed = false, native = false, revision = 0;
    const stops: Array<() => void> = [];
    const media = matchMedia("(prefers-color-scheme: dark)");
    const update = (value: Theme | null) => {
      if (disposed) return;
      native = value !== null;
      setTheme(value ?? (media.matches ? "dark" : "light"));
    };
    const syncMedia = () => { if (!native) update(null); };
    media.addEventListener("change", syncMedia);
    const retain = (stop: () => void) => { if (disposed) stop(); else stops.push(stop); };
    if (isTauri()) {
      void (async () => {
        try {
          if (nativeLinuxTitlebar) {
            retain(await listen<Theme | null>("desktop-theme-changed", ({ payload }) => { revision++; update(payload); }));
            const initial = revision;
            const value = await invoke<Theme | null>("get_desktop_theme");
            if (revision === initial) update(value);
          } else {
            const window = getCurrentWindow();
            retain(await window.onThemeChanged(({ payload }) => { revision++; update(payload); }));
            const initial = revision;
            const value = await window.theme();
            if (revision === initial) update(value);
          }
        } catch (error) { console.warn("Desktop appearance unavailable; using system media preference", error); }
      })();
    }
    return () => { disposed = true; stops.forEach(stop => stop()); media.removeEventListener("change", syncMedia); };
  }, []);
  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    document.documentElement.style.colorScheme = theme;
  }, [theme]);
  return theme;
}
