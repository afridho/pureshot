import { invoke, isTauri } from "@tauri-apps/api/core";
import type { CaptureItem, CaptureMode } from "./types";

export type CaptureSettings = {
  cornerRadius: number;
  previewTimeoutSecs: number;
  copyAfterCapture: boolean;
  hideDesktopIcons: boolean;
  showQuickAccess: boolean;
  quickAccessHover: boolean;
  showCrosshair: boolean;
  showMagnifier: boolean;
  freezeScreen: boolean;
};

export function readCaptureSettings(): CaptureSettings {
  const boolean = (key: string, fallback: boolean) => (localStorage.getItem(key) ?? String(fallback)) === "true";
  const number = (key: string, fallback: number, max: number) => {
    const value = Number(localStorage.getItem(key) ?? fallback);
    return Number.isFinite(value) && value >= 0 && value <= max ? value : fallback;
  };
  return {
    cornerRadius: number("cornerRadius", 13, 32),
    previewTimeoutSecs: number("previewTimeout", 10, 30),
    copyAfterCapture: boolean("copyAfterCapture", false) || boolean("copyOnCapture", false),
    hideDesktopIcons: boolean("hideDesktopIcons", false),
    showQuickAccess: boolean("showQuickAccess", true),
    quickAccessHover: boolean("quickAccessHover", true),
    showCrosshair: boolean("showCrosshair", true),
    showMagnifier: boolean("showMagnifier", true),
    freezeScreen: boolean("freezeScreen", false),
  };
}

export async function updateCaptureSettings(settings: CaptureSettings) {
  if (isTauri()) await invoke("update_capture_settings", settings);
  for (const [key, value] of Object.entries(settings)) {
    localStorage.setItem(key === "previewTimeoutSecs" ? "previewTimeout" : key, String(value));
  }
  localStorage.removeItem("copyOnCapture");
  document.documentElement.style.setProperty("--capture-radius", `${settings.cornerRadius}px`);
  window.dispatchEvent(new CustomEvent("pureshot-settings-changed"));
}

export async function capture(mode: CaptureMode): Promise<CaptureItem | null> {
  if (isTauri()) {
    const cornerRadius = Number(localStorage.getItem("cornerRadius") ?? "13");
    const previewTimeoutSecs = Number(localStorage.getItem("previewTimeout") ?? "10");
    if (mode === "area") {
      await invoke("start_area_capture", { cornerRadius, previewTimeoutSecs });
      return null;
    }
    const item = await invoke<CaptureItem>("capture_screen", { mode });
    await invoke("show_automatic_capture_overlay", { id: item.id, cornerRadius, previewTimeoutSecs });
    return item;
  }
  await new Promise((r) => setTimeout(r, 350));
  return {
    id: crypto.randomUUID(), path: "Preview capture.png", width: 1440, height: 900,
    createdAt: Date.now(), dataUrl: `https://images.unsplash.com/photo-1497366754035-f200968a6e72?w=1200&auto=format&fit=crop`
  };
}

export async function performAction(action: string, item: CaptureItem) {
  if (isTauri()) {
    if (action === "pin") {
      const cornerRadius = Number(localStorage.getItem("cornerRadius") ?? "13");
      return invoke("pin_capture", { id: item.id, cornerRadius });
    }
    return invoke("capture_action", { action, item });
  }
  console.info(action, item.path);
}

