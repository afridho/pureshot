import { useEffect, useRef, useState, type ReactNode, type WheelEvent as ReactWheelEvent } from "react";
import { listen } from "@tauri-apps/api/event";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Camera, Check, ChevronDown, Cloud, Command, Download, Folder, GripHorizontal, Info, Maximize, MousePointer2, Palette, Pencil, Pin, Scan, Settings, X, Zap } from "./icons";
import { capture, performAction, readCaptureSettings, updateCaptureSettings, type CaptureSettings } from "./tauri";
import AnnotationEditor from "./AnnotationEditor";
import OcrView, { OcrSetup } from "./OcrView";
import FilenameSettings from "./FilenameSettings";
import { useDesktopAppearance } from "./useDesktopAppearance";
import appIcon from "../icon.png";
import type { CaptureItem, CaptureMode } from "./types";

const modes = [
  { id: "area", label: "Capture area", hint: "Select any part of your screen", icon: Scan, key: "Shift Alt 4" },
  { id: "window", label: "Capture window", hint: "Pick a window with pixel precision", icon: MousePointer2, key: "Shift Alt 5" },
  { id: "fullscreen", label: "Capture fullscreen", hint: "Capture your entire display", icon: Maximize, key: "Shift Alt 3" },
  { id: "ocr", label: "Capture OCR", hint: "Select an area and copy recognized text or QR content. Text recognition requires installed OCR models.", icon: Scan, key: "" },
] as const;

function App() {
  useDesktopAppearance();
  const [current, setCurrent] = useState<CaptureItem | null>(null);
  const [history, setHistory] = useState<CaptureItem[]>([]);
  const [busy, setBusy] = useState(false);
  const captureBusy = useRef(false);
  const [captureError, setCaptureError] = useState<string | null>(null);
  const view = new URLSearchParams(location.search).get("view");
  const overlay = view === "overlay" || view === "pin" || view === "overlay-stack";
  const annotation = view === "annotate";
  const ocr = view === "ocr";
  const historyView = view === "history";

  async function run(mode: CaptureMode) {
    if (captureBusy.current) return;
    captureBusy.current = true;
    setBusy(true);
    setCaptureError(null);
    try {
      const item = await capture(mode);
      if (item) {
        setHistory((old) => [item, ...old].slice(0, 20));
        if (!(window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__) setCurrent(item);
      }
    } catch (error) {
      if (String(error) !== "Screenshot cancelled") setCaptureError(String(error));
    } finally { captureBusy.current = false; setBusy(false); }
  }

  useEffect(() => {
    if ((overlay && view !== "overlay-stack") || annotation) {
      void invoke<CaptureItem | null>("get_window_capture")
        .then((item) => { if (item) setCurrent(item); }).catch(console.error);
    }
    if (historyView) {
      void invoke<CaptureItem[]>("get_capture_history").then(setHistory).catch(console.error);
    }
    const captureListener = listen<CaptureItem>("capture-created", ({ payload }) => {
      if (!overlay && !annotation && !ocr) setHistory((old) => [payload, ...old].slice(0, 20));
    });
    const errorListener = listen<string>("capture-error", ({ payload }) => setCaptureError(payload));
    const shortcutListener = listen<CaptureMode>("shortcut-capture", ({ payload }) => { if (!overlay && !annotation && !ocr && !historyView) void run(payload); });
    const cliCaptureListener = listen("cli-capture", () => {
      if (!overlay && !annotation && !ocr && !historyView) void invoke("process_cli_capture").catch(console.error);
    });
    // A CLI action on a fresh launch is queued in Tauri setup before this listener mounts.
    if (!overlay && !annotation && !ocr && !historyView) void invoke("process_cli_capture").catch(console.error);
    return () => {
      void errorListener.then((f) => f());
      void captureListener.then((f) => f());
      void shortcutListener.then((f) => f());
      void cliCaptureListener.then((f) => f());
    };
  }, []);

  if (ocr) return <OcrView/>;
  if (view === "overlay-stack") return <QuickAccessStack/>;
  if (annotation) return current ? <AnnotationEditor item={current}/> : <div className="annotation-loading">Loading screenshot…</div>;
  if (historyView) return <HistoryOverlay items={history} />;
  if (overlay) return current
    ? <div className="capture-webview"><QuickOverlay item={current} pinned={view === "pin"} close={() => void getCurrentWindow().close()} /></div>
    : <div className="overlay-loading" />;

  return <main className="settings-window">
    <div className="custom-titlebar" data-tauri-drag-region>
      <div className="window-controls">
        <button type="button" className="window-close" title="Hide to tray" aria-label="Hide PureShot to tray" onClick={() => void getCurrentWindow().hide().catch(console.error)}><span className="settings-close-icon" aria-hidden="true"/></button>
      </div>
    </div>
    <FeaturePage page="settings" history={history} captureError={captureError} />
    {current && <QuickOverlay item={current} close={() => setCurrent(null)} />}
  </main>
}

type SettingsSection = "general" | "shortcuts" | "quick-access" | "screenshots" | "advanced" | "about";

function FeaturePage({ page, captureError }: { page: "settings"; history: CaptureItem[]; captureError: string | null }) {
  const [section, setSection] = useState<SettingsSection>("general");
  const [exportDirectory, setExportDirectory] = useState("");
  const [exportBusy, setExportBusy] = useState(true);
  const [exportError, setExportError] = useState<string | null>(null);
  useEffect(() => {
    if (!isTauri()) return;
    let active = true;
    void invoke<string>("get_export_directory")
      .then(path => { if (active) setExportDirectory(path); })
      .catch(error => { if (active) setExportError(`Unable to read export folder: ${String(error)}`); })
      .finally(() => { if (active) setExportBusy(false); });
    return () => { active = false; };
  }, []);
  const chooseExportDirectory = async () => {
    setExportBusy(true); setExportError(null);
    try {
      const path = await invoke<string | null>("choose_export_directory");
      if (path !== null) setExportDirectory(path);
    } catch (error) {
      setExportError(`Unable to change export folder: ${String(error)}`);
    } finally { setExportBusy(false); }
  };
  const [defaultFormat, setDefaultFormat] = useState("PNG");
  const [formatBusy, setFormatBusy] = useState(true);
  const [formatError, setFormatError] = useState<string | null>(null);
  useEffect(() => {
    if (!isTauri()) { setFormatBusy(false); return; }
    let active = true;
    void invoke<string>("get_default_format")
      .then(format => { if (active) setDefaultFormat(format); })
      .catch(error => { if (active) setFormatError(`Unable to read default format: ${String(error)}`); })
      .finally(() => { if (active) setFormatBusy(false); });
    return () => { active = false; };
  }, []);
  const changeDefaultFormat = async (format: string) => {
    setFormatBusy(true); setFormatError(null);
    try {
      await invoke("set_default_format", { format });
      setDefaultFormat(format);
    } catch (error) {
      setFormatError(`Unable to change default format: ${String(error)}`);
    } finally { setFormatBusy(false); }
  };
  const [showTrayIcon, setShowTrayIcon] = useState(true);
  const [trayBusy, setTrayBusy] = useState(true);
  const [trayError, setTrayError] = useState<string | null>(null);
  useEffect(() => {
    if (!isTauri()) return;
    let active = true;
    void invoke<boolean>("get_show_tray_icon")
      .then((visible) => { if (active) setShowTrayIcon(visible); })
      .catch((error) => { if (active) setTrayError(`Unable to read tray setting: ${String(error)}`); })
      .finally(() => { if (active) setTrayBusy(false); });
    return () => { active = false; };
  }, []);
  const changeShowTrayIcon = async (visible: boolean) => {
    setTrayBusy(true);
    setTrayError(null);
    try {
      await invoke("set_show_tray_icon", { visible });
      setShowTrayIcon(visible);
    } catch (error) {
      setTrayError(`Unable to change tray setting: ${String(error)}`);
    } finally {
      setTrayBusy(false);
    }
  };
  const [launchAtStartup, setLaunchAtStartup] = useState(false);
  const [startupBusy, setStartupBusy] = useState(true);
  const [startupError, setStartupError] = useState<string | null>(null);
  useEffect(() => {
    if (!isTauri()) {
      setStartupError("Launch at startup is available in the desktop app only.");
      return;
    }
    let active = true;
    void invoke<boolean>("get_launch_at_startup")
      .then((enabled) => { if (active) setLaunchAtStartup(enabled); })
      .catch((error) => { if (active) setStartupError(`Unable to read startup setting: ${String(error)}`); })
      .finally(() => { if (active) setStartupBusy(false); });
    return () => { active = false; };
  }, []);
  const changeLaunchAtStartup = async (enabled: boolean) => {
    setStartupBusy(true);
    setStartupError(null);
    try {
      const actual = await invoke<boolean>("set_launch_at_startup", { enabled });
      setLaunchAtStartup(actual);
      if (actual !== enabled) setStartupError("The desktop did not apply the startup setting. Please try again.");
    } catch (error) {
      setStartupError(`Unable to change startup setting: ${String(error)}`);
    } finally {
      setStartupBusy(false);
    }
  };
  const [captureSettings, setCaptureSettings] = useState(readCaptureSettings);
  const [draftRadius, setDraftRadius] = useState(captureSettings.cornerRadius);
  const [settingsBusy, setSettingsBusy] = useState(true);
  const settingsSaving = useRef(false);
  const [settingsError, setSettingsError] = useState<string | null>(null);
  useEffect(() => {
    let active = true;
    settingsSaving.current = true;
    void updateCaptureSettings(captureSettings)
      .catch(error => { if (active) setSettingsError(`Unable to apply capture settings: ${String(error)}`); })
      .finally(() => { settingsSaving.current = false; if (active) setSettingsBusy(false); });
    return () => { active = false; };
  }, []);
  const changeCaptureSettings = async (patch: Partial<CaptureSettings>) => {
    if (settingsSaving.current) return;
    settingsSaving.current = true;
    setSettingsBusy(true);
    setSettingsError(null);
    const next = { ...captureSettings, ...patch };
    try {
      await updateCaptureSettings(next);
      setCaptureSettings(next);
    } catch (error) {
      if (patch.cornerRadius !== undefined) setDraftRadius(captureSettings.cornerRadius);
      setSettingsError(`Unable to save capture settings: ${String(error)}`);
    } finally {
      settingsSaving.current = false;
      setSettingsBusy(false);
    }
  };
  const [hardwareAcceleration, setHardwareAcceleration] = useState(true);
  const [hardwareBusy, setHardwareBusy] = useState(true);
  const [hardwareError, setHardwareError] = useState<string | null>(null);
  useEffect(() => {
    if (!isTauri()) { setHardwareBusy(false); return; }
    let active = true;
    void invoke<boolean>("get_hardware_acceleration")
      .then(enabled => { if (active) setHardwareAcceleration(enabled); })
      .catch(error => { if (active) setHardwareError(`Unable to read hardware acceleration: ${String(error)}`); })
      .finally(() => { if (active) setHardwareBusy(false); });
    return () => { active = false; };
  }, []);
  const changeHardwareAcceleration = async (enabled: boolean) => {
    setHardwareBusy(true);
    setHardwareError(null);
    try {
      await invoke("set_hardware_acceleration", { enabled });
      setHardwareAcceleration(enabled);
    } catch (error) {
      setHardwareError(`Unable to save hardware acceleration: ${String(error)}`);
    } finally { setHardwareBusy(false); }
  };
  const [shortcuts, setShortcuts] = useState<Record<string, string>>(() => Object.fromEntries(modes.map(mode => [mode.id, mode.key])));
  const [shortcutError, setShortcutError] = useState<string | null>(null);
  const [recordingMode, setRecordingMode] = useState<string | null>(null);
  const [shortcutBusy, setShortcutBusy] = useState(true);
  const shortcutSaving = useRef(false);
  useEffect(() => {
    if (!isTauri()) return;
    let active = true;
    void invoke<Record<string, string>>("get_capture_shortcuts")
      .then((value) => { if (active) setShortcuts(old => ({ ...old, ...Object.fromEntries(Object.entries(value).map(([mode, accelerator]) => [mode, accelerator.replace(/\+/g, " ")])) })); })
      .catch((error) => { if (active) setShortcutError(`Unable to read shortcuts: ${String(error)}`); })
      .finally(() => { if (active) setShortcutBusy(false); });
    return () => { active = false; };
  }, []);
  const saveShortcut = async (mode: string, accelerator: string) => {
    if (shortcutSaving.current) return;
    shortcutSaving.current = true;
    setShortcutBusy(true);
    setRecordingMode(null);
    setShortcutError(null);
    try {
      await invoke("set_capture_shortcut", { mode, accelerator });
      setShortcuts(old => ({ ...old, [mode]: accelerator.replace(/\+/g, " ") }));
    } catch (error) {
      setShortcutError(`Unable to change shortcut: ${String(error)}`);
    } finally {
      shortcutSaving.current = false;
      setShortcutBusy(false);
    }
  };
  const recordShortcut = (mode: string) => {
    if (shortcutBusy) return;
    setShortcutError(null);
    setRecordingMode(mode);
  };
  useEffect(() => {
    if (!recordingMode) return;
    const modifierCodes = new Set(["ControlLeft", "ControlRight", "ShiftLeft", "ShiftRight", "AltLeft", "AltRight", "MetaLeft", "MetaRight"]);
    const handleKeyDown = async (event: KeyboardEvent) => {
      if (event.code === "Tab") { setRecordingMode(null); return; }
      event.preventDefault();
      event.stopPropagation();
      if (event.repeat || shortcutSaving.current) return;
      if (event.code === "Escape") { setRecordingMode(null); return; }
      if (modifierCodes.has(event.code)) return;
      const modifiers: string[] = [];
      if (event.ctrlKey) modifiers.push("Ctrl");
      if (event.shiftKey) modifiers.push("Shift");
      if (event.altKey) modifiers.push("Alt");
      if (event.metaKey) modifiers.push("Super");
      // event.code is the physical key, unaffected by Shift/AltGr remapping
      // the character (e.g. Shift+4 -> "$" in event.key on many layouts).
      const key = event.code.replace(/^Digit/, "").replace(/^Key/, "");
      if (modifiers.length === 0) { setShortcutError("Shortcuts need at least one modifier key (Ctrl, Alt, Shift or Super)."); return; }
      const accelerator = [...modifiers, key].join("+");
      await saveShortcut(recordingMode, accelerator);
    };
    window.addEventListener("keydown", handleKeyDown, true);
    return () => window.removeEventListener("keydown", handleKeyDown, true);
  }, [recordingMode]);
  const sections = [
    { id: "general", label: "General", icon: Settings },
    { id: "shortcuts", label: "Shortcuts", icon: Command },
    { id: "quick-access", label: "Quick Access", icon: Zap },
    { id: "screenshots", label: "Screenshots", icon: Camera },
    { id: "advanced", label: "Advanced", icon: Palette },
  ] as const;
  const toggle = (key: "hideDesktopIcons" | "showQuickAccess" | "quickAccessHover" | "showCrosshair" | "showMagnifier" | "freezeScreen" | "copyAfterCapture") => <input className="switch" type="checkbox" checked={captureSettings[key]} disabled={settingsBusy} aria-busy={settingsBusy} onChange={(event) => void changeCaptureSettings({ [key]: event.target.checked })}/>;
  const setting = (title: string, description: string | undefined, control: ReactNode) => <label className="setting-row"><span><strong>{title}</strong>{description && <small>{description}</small>}</span>{control}</label>;

  return <div className="settings-workspace">
    <aside className="settings-sidebar">
      <div className="settings-profile"><img src={appIcon} alt="PureShot"/><span><strong>PureShot</strong><small>Settings</small></span></div>
      <nav>{sections.map(({ id, label, icon: Icon }) => <button key={id} className={section === id ? "active" : ""} onClick={() => setSection(id)}><Icon size={17}/>{label}</button>)}</nav>
      <button className={`about-link ${section === "about" ? "active" : ""}`} onClick={() => setSection("about")}><Info size={17}/>About</button>
    </aside>
    <div className="settings-content">
      <header><div><h1>{sections.find(item => item.id === section)?.label ?? "About"}</h1><p>Customize how PureShot works for you.</p></div></header>
      {section === "general" && <><SettingsGroup title="App">{setting("Launch at startup", startupError ?? "Keep PureShot ready in the system tray.", <input className="switch" type="checkbox" checked={launchAtStartup} disabled={startupBusy || !isTauri()} aria-busy={startupBusy} onChange={(event) => void changeLaunchAtStartup(event.target.checked)}/>)}{setting("Show system tray icon", trayError ?? "Hide the tray icon without disabling capture shortcuts.", <input className="switch" type="checkbox" checked={showTrayIcon} disabled={trayBusy || !isTauri()} aria-busy={trayBusy} onChange={(event) => void changeShowTrayIcon(event.target.checked)}/>)}</SettingsGroup><SettingsGroup title="Capture">{setting("Hide desktop icons while capturing", "Temporarily hide desktop icons for capture. On GNOME, requires the updated companion and is unavailable for portal window capture.", toggle("hideDesktopIcons"))}</SettingsGroup><SettingsGroup title="Export">{setting("Export location", exportError ?? "Default location used by Save actions.", <button type="button" className="choose-button" style={{ maxWidth: "55%", minWidth: 0 }} title={exportDirectory || "Choose export folder"} disabled={exportBusy || !isTauri()} aria-busy={exportBusy} onClick={() => void chooseExportDirectory()}><Folder size={14}/><span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{exportBusy ? "Loading…" : exportDirectory || "Choose folder…"}</span></button>)}{setting("Default format", formatError ?? "Used by Save screenshot and annotations. JPEG uses a white background for transparency.", <ModernSelect value={defaultFormat} disabled={formatBusy || !isTauri()} options={[{ value: "PNG", label: "PNG" }, { value: "JPEG", label: "JPEG" }, { value: "WebP", label: "WebP" }]} onChange={format => void changeDefaultFormat(format)}/>)}<FilenameSettings defaultFormat={defaultFormat}/></SettingsGroup></>}
      {captureError && <p role="alert">{captureError}</p>}
      {settingsError && <p className="settings-note settings-note-error" role="alert">{settingsError}</p>}
      {section === "shortcuts" && <SettingsGroup title="Capture shortcuts">{modes.map(mode => <div className="setting-row" key={mode.id}><span><strong>{mode.label}</strong><small>{mode.hint}</small></span><div className="shortcut-controls"><button type="button" disabled={shortcutBusy || !isTauri()} className={`shortcut-picker${recordingMode === mode.id ? " is-recording" : ""}`} aria-label={`Change shortcut for ${mode.label}`} aria-pressed={recordingMode === mode.id} onClick={() => recordShortcut(mode.id)}><kbd>{recordingMode === mode.id ? "Press keys…" : shortcuts[mode.id] || "Not assigned"}</kbd></button>{recordingMode === mode.id ? <button type="button" className="shortcut-clear" onClick={() => setRecordingMode(null)}>Cancel</button> : <button type="button" className="shortcut-clear" disabled={shortcutBusy || !isTauri() || !shortcuts[mode.id]} aria-label={`Clear shortcut for ${mode.label}`} onClick={() => void saveShortcut(mode.id, "")}>Clear</button>}</div></div>)}{shortcutError && <p className="settings-note settings-note-error">{shortcutError}</p>}<p className="settings-note">On GNOME Wayland, area capture requires the PureShot companion extension and uses PureShot’s selector without the desktop picker. Enable Freeze screen in Screenshots to select against frozen content. Fullscreen and window capture use the desktop portal. Global shortcuts and always-on-top windows depend on your desktop; click a shortcut above to change it, or Clear to leave it unassigned. Press Escape to cancel recording. OCR models can be installed in Screenshots → OCR.</p></SettingsGroup>}
      {section === "quick-access" && <><SettingsGroup title="Quick Access Overlay">{setting("Show after capture", "Display a preview after every screenshot.", toggle("showQuickAccess"))}{setting("Auto-close", "How long the preview remains visible.", <ModernSelect value={String(captureSettings.previewTimeoutSecs)} disabled={settingsBusy} options={[{ value: "3", label: "3 seconds" }, { value: "5", label: "5 seconds" }, { value: "10", label: "10 seconds" }, { value: "15", label: "15 seconds" }, { value: "30", label: "30 seconds" }, { value: "0", label: "Never" }]} onChange={(next) => void changeCaptureSettings({ previewTimeoutSecs: Number(next) })}/>)}{setting("Show on hover", "Reveal quick actions when the pointer enters the preview.", toggle("quickAccessHover"))}</SettingsGroup><SettingsGroup title="Appearance">{setting("Corner radius", "Applied to previews, floating screenshots, and OCR selection.", <span className="range-setting"><input type="range" min="0" max="32" value={draftRadius} disabled={settingsBusy} onChange={(event) => setDraftRadius(Number(event.target.value))} onPointerDown={(event) => event.currentTarget.setPointerCapture(event.pointerId)} onPointerUp={(event) => { const radius = Number(event.currentTarget.value); if (radius !== captureSettings.cornerRadius) void changeCaptureSettings({ cornerRadius: radius }); }} onPointerCancel={() => setDraftRadius(captureSettings.cornerRadius)} onBlur={(event) => { const radius = Number(event.currentTarget.value); if (radius !== captureSettings.cornerRadius) void changeCaptureSettings({ cornerRadius: radius }); }}/><b>{draftRadius}px</b></span>)}</SettingsGroup></>}
      {section === "screenshots" && <><SettingsGroup title="Capture">{setting("Copy to clipboard after capture", "Automatically copy every new screenshot to the clipboard.", toggle("copyAfterCapture"))}{setting("Show crosshair", "Display precise guides while selecting an area.", toggle("showCrosshair"))}{setting("Show magnifier", "Magnify pixels near the selection cursor.", toggle("showMagnifier"))}{setting("Freeze screen", "Freeze visible content while selecting an area.", toggle("freezeScreen"))}</SettingsGroup><SettingsGroup title="OCR"><OcrSetup captureButton/></SettingsGroup><SettingsGroup title="Window screenshots">{setting("Include window shadow", "Coming soon", <input className="switch" type="checkbox" checked={false} disabled/>)}</SettingsGroup></>}
      {section === "advanced" && <SettingsGroup title="Behavior">{setting("Hardware acceleration", "Use accelerated rendering for app webviews. On Linux, disabling turns off accelerated compositing; some GPU features may remain active. Restart app required.", <input className="switch" type="checkbox" checked={hardwareAcceleration} disabled={hardwareBusy || !isTauri()} aria-busy={hardwareBusy} onChange={(event) => void changeHardwareAcceleration(event.target.checked)}/>)}{hardwareError && <p className="settings-note settings-note-error" role="alert">{hardwareError}</p>}</SettingsGroup>}
      {section === "about" && <div className="about-card"><img src={appIcon} alt="PureShot"/><h2>PureShot</h2><p>A fast, private screenshot utility for Windows and Linux.</p><span>Version 0.1.0</span></div>}
    </div>
  </div>;
}

function SettingsGroup({ title, children }: { title: string; children: ReactNode }) {
  return <section className="settings-group"><h2>{title}</h2><div>{children}</div></section>;
}

type SelectOption = { value: string; label: string };

function ModernSelect({ value, defaultValue, options, onChange, disabled = false }: { value?: string; defaultValue?: string; options: SelectOption[]; onChange?: (value: string) => void; disabled?: boolean }) {
  const [internal, setInternal] = useState(defaultValue ?? options[0]?.value ?? "");
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  const selected = value ?? internal;
  useEffect(() => {
    if (!open) return;
    const close = (event: PointerEvent) => { if (!root.current?.contains(event.target as Node)) setOpen(false); };
    const escape = (event: KeyboardEvent) => { if (event.key === "Escape") setOpen(false); };
    document.addEventListener("pointerdown", close);
    document.addEventListener("keydown", escape);
    return () => { document.removeEventListener("pointerdown", close); document.removeEventListener("keydown", escape); };
  }, [open]);
  const choose = (next: string) => {
    setInternal(next);
    onChange?.(next);
    setOpen(false);
  };
  return <div className={`modern-select ${open ? "open" : ""}`} ref={root}>
    <button type="button" className="select-trigger" disabled={disabled} aria-haspopup="listbox" aria-expanded={open && !disabled} onClick={() => setOpen(current => !current)}>
      {options.find(option => option.value === selected)?.label ?? selected}<ChevronDown size={14}/>
    </button>
    {open && !disabled && <div className="select-menu" role="listbox">{options.map(option => <button type="button" role="option" aria-selected={option.value === selected} className={option.value === selected ? "selected" : ""} key={option.value} onClick={() => choose(option.value)}><span>{option.label}</span>{option.value === selected && <span className="select-check">✓</span>}</button>)}</div>}
  </div>;
}

function HistoryOverlay({ items }: { items: CaptureItem[] }) {
  const [selected, setSelected] = useState(0);
  const wheelDelta = useRef(0);
  const [ocrBusy, setOcrBusy] = useState(false);
  const [ocrError, setOcrError] = useState<string | null>(null);
  const extractText = async () => {
    const item = items[selected];
    if (!item || ocrBusy) return;
    setOcrBusy(true); setOcrError(null);
    try { await invoke("show_ocr_capture", { id: item.id }); }
    catch (error) { setOcrError(String(error)); }
    finally { setOcrBusy(false); }
  };
  const restore = async () => {
    const item = items[selected];
    if (item) await invoke("restore_history_capture", { id: item.id });
  };
  useEffect(() => {
    const close = () => { void getCurrentWindow().close(); };
    const keydown = (event: KeyboardEvent) => {
      if (event.key === "Escape" || event.key === "Esc" || event.code === "Escape") {
        event.preventDefault();
        event.stopImmediatePropagation();
        close();
      } else if (event.key === "ArrowLeft") setSelected(value => Math.max(0, value - 1));
      else if (event.key === "ArrowRight") setSelected(value => Math.min(items.length - 1, value + 1));
      else if (event.key === "Enter") void restore();
      else if (event.key === "Delete" && items[selected]) {
        void invoke("delete_history_capture", { id: items[selected].id }).then(() => location.reload());
      }
    };
    window.addEventListener("keydown", keydown, true);
    return () => window.removeEventListener("keydown", keydown, true);
  }, [items, selected]);
  const scrollHistory = (event: ReactWheelEvent) => {
    event.preventDefault();
    const delta = Math.abs(event.deltaX) > Math.abs(event.deltaY) ? event.deltaX : event.deltaY;
    wheelDelta.current += delta;
    if (Math.abs(wheelDelta.current) < 36) return;
    const direction = wheelDelta.current > 0 ? 1 : -1;
    wheelDelta.current = 0;
    setSelected(value => Math.max(0, Math.min(items.length - 1, value + direction)));
  };
  return <main className="history-overlay">
    <div className="history-panel" onWheel={scrollHistory}>
      <section className="history-strip" style={{ transform: `translateX(calc(50vw - ${120 + selected * 256}px))` }}>
        {items.map((item, index) => <button type="button" key={item.id} className={`history-card ${index === selected ? "selected" : ""}`} onClick={() => setSelected(index)} onDoubleClick={() => void restore()}><img src={item.dataUrl}/></button>)}
      </section>
      {items.length ? <><div className="ocr-history-actions"><button type="button" className="history-restore" onClick={() => void restore()}>↩&nbsp; Restore</button><button type="button" className="history-restore" disabled={ocrBusy || !items[selected]} onClick={() => void extractText()}>{ocrBusy ? "Opening…" : "Extract Text"}</button></div><div className="history-filename">{items[selected]?.path.split(/[\\/]/).pop()}</div>{ocrError && <p className="ocr-history-error" role="alert">{ocrError}</p>}</> : <div className="history-empty">No recent captures</div>}
    </div>
  </main>;
}

type QuickAccessPreview = { item: CaptureItem; timeout: number; cornerRadius: number; hover: boolean };
type QuickAccessShelf = { revision: number; entries: QuickAccessPreview[]; capacity: number };

function QuickAccessStack() {
  const [shelf, setShelf] = useState<QuickAccessShelf>({ revision: -1, entries: [], capacity: 1 });
  useEffect(() => {
    let active = true;
    const apply = (next: QuickAccessShelf) => {
      if (active) setShelf(previous => next.revision >= previous.revision ? next : previous);
    };
    // Listen before fetching: the versioned snapshot also covers captures
    // arriving during WebView startup, without duplicate cards or stale state.
    const listener = listen<QuickAccessShelf>("quick-access-changed", ({ payload }) => apply(payload));
    void listener.then(() => invoke<QuickAccessShelf>("get_quick_access")).then(apply).catch(console.error);
    return () => { active = false; void listener.then(unlisten => unlisten()); };
  }, []);
  const dismiss = (id: string) => { void invoke("dismiss_quick_access", { id }).catch(console.error); };
  return <main className="quick-access-stack" aria-label="Recent screenshots">
    {shelf.entries.map((entry, index) => <div className="quick-access-slot" key={entry.item.id}
      style={{ bottom: `${24 + index * 222}px` }}>
      <QuickOverlay item={entry.item} close={() => dismiss(entry.item.id)} stackPreview={entry}/>
    </div>)}
  </main>;
}

function QuickOverlay({item, close, pinned = false, stackPreview}:{item:CaptureItem; close:()=>void; pinned?:boolean; stackPreview?:QuickAccessPreview}) {
  const [hovered, setHovered] = useState(false);
  const [focused, setFocused] = useState(false);
  const pointerInteraction = useRef(false);
  useEffect(() => {
    if (!stackPreview) return;
    const leave = () => {
      setHovered(false);
      setFocused(false);
      if (document.activeElement instanceof HTMLElement) document.activeElement.blur();
    };
    // WebKit may retain :hover when crossing a shaped transparent window's
    // input boundary. GTK's leave event is authoritative in that case.
    const pointerListener = listen<string | null>("quick-access-pointer-card", ({ payload }) => {
      if (payload === item.id) setHovered(true);
      else leave();
    });
    return () => {
      void pointerListener.then(unlisten => unlisten());
    };
  }, []);
  const readHoverSetting = () => (new URLSearchParams(location.search).get("hover") ?? localStorage.getItem("quickAccessHover") ?? "true") === "true";
  const [hoverActions, setHoverActions] = useState(() => stackPreview?.hover ?? readHoverSetting());
  const [isPinned, setIsPinned] = useState(pinned);
  const [closing, setClosing] = useState(false);
  const [opacity, setOpacity] = useState(100);
  const [completedAction, setCompletedAction] = useState<"copy" | "save" | null>(null);
  const feedbackTimer = useRef<number | null>(null);
  const [radius, setRadius] = useState(() => stackPreview?.cornerRadius ?? Number(localStorage.getItem("cornerRadius") ?? "13"));
  const autoCloseRemaining = useRef((stackPreview?.timeout ?? Number(new URLSearchParams(location.search).get("timeout") ?? localStorage.getItem("previewTimeout") ?? "10")) * 1000);
  const autoCloseStarted = useRef<number | null>(null);
  const closeRef = useRef(close);
  closeRef.current = close;
  const closingRef = useRef(false);
  const closeTimer = useRef<number | null>(null);
  const animatedClose = () => {
    if (closingRef.current) return;
    if (isPinned) { closeRef.current(); return; }
    closingRef.current = true;
    setClosing(true); closeTimer.current = window.setTimeout(() => closeRef.current(), 260);
  };
  const runAction = async (action: "copy" | "save") => {
    try {
      await performAction(action, item);
      setCompletedAction(action);
      if (feedbackTimer.current !== null) window.clearTimeout(feedbackTimer.current);
      feedbackTimer.current = window.setTimeout(() => setCompletedAction(null), 1400);
    } catch (error) {
      console.error(`${action} failed`, error);
    }
  };
  useEffect(() => {
    const pinListener = stackPreview ? null : listen("pin-activated", () => setIsPinned(true));
    const syncSettings = () => {
      setRadius(Number(localStorage.getItem("cornerRadius") ?? "13"));
      setHoverActions((localStorage.getItem("quickAccessHover") ?? "true") === "true");
    };
    window.addEventListener("storage", syncSettings);
    window.addEventListener("pureshot-settings-changed", syncSettings);
    return () => {
      if (pinListener) void pinListener.then((unlisten) => unlisten());
      window.removeEventListener("storage", syncSettings);
      window.removeEventListener("pureshot-settings-changed", syncSettings);
      if (feedbackTimer.current !== null) window.clearTimeout(feedbackTimer.current);
      if (closeTimer.current !== null) window.clearTimeout(closeTimer.current);
    };
  }, []);
  useEffect(() => {
    const dismiss = (event: KeyboardEvent) => {
      if (event.key === "Escape" && (!stackPreview || hovered || focused)) { event.preventDefault(); animatedClose(); }
    };
    window.addEventListener("keydown", dismiss);
    return () => window.removeEventListener("keydown", dismiss);
  }, [hovered, focused, isPinned]);
  useEffect(() => {
    if (isPinned || hovered || (!stackPreview && focused) || closingRef.current || autoCloseRemaining.current <= 0) return;
    autoCloseStarted.current = performance.now();
    const timer = window.setTimeout(animatedClose, autoCloseRemaining.current);
    return () => {
      window.clearTimeout(timer);
      if (autoCloseStarted.current !== null) {
        autoCloseRemaining.current = Math.max(0, autoCloseRemaining.current - (performance.now() - autoCloseStarted.current));
        autoCloseStarted.current = null;
      }
    };
  }, [hovered, focused, isPinned]);
  useEffect(() => {
    if (!isPinned) return;
    const handleOpacityGesture = (event: WheelEvent) => {
      // Pinch is consumed by the native Windows Precision Touchpad handler.
      // Ordinary two-finger vertical movement remains CleanShot-style opacity.
      if (event.ctrlKey) return;
      event.preventDefault();
      setOpacity((value) => Math.min(100, Math.max(20, value + (event.deltaY < 0 ? 5 : -5))));
    };
    window.addEventListener("wheel", handleOpacityGesture, { passive: false, capture: true });
    return () => window.removeEventListener("wheel", handleOpacityGesture, true);
  }, [isPinned]);
  return <div className={`quick-overlay ${hovered || (!stackPreview && focused) ? "actions-revealed" : ""} ${closing ? "is-closing" : ""} ${isPinned ? "is-pinned" : ""} ${!hoverActions ? "always-show-actions" : ""}`} style={{ "--capture-radius": `${radius}px`, opacity: closing ? 0 : opacity / 100 } as React.CSSProperties} onPointerDown={() => { pointerInteraction.current = true; }} onKeyDown={() => { pointerInteraction.current = false; }} onFocus={() => setFocused(!pointerInteraction.current)} onBlur={(event) => { if (!event.currentTarget.contains(event.relatedTarget as Node | null)) setFocused(false); }} onMouseDown={(event) => { if (isPinned && !(event.target as HTMLElement).closest("button,.pin-controls")) void getCurrentWindow().startDragging(); }} onPointerMove={() => { if (!stackPreview) setHovered(true); }} onMouseEnter={() => { if (!stackPreview) setHovered(true); }} onPointerLeave={(event) => {
    if (stackPreview) return;
    setHovered(false);
    if (stackPreview) {
      setFocused(false);
      const active = document.activeElement;
      if (active instanceof HTMLElement && event.currentTarget.contains(active)) active.blur();
    }
  }} onMouseLeave={(event) => {
    if (stackPreview) return;
    setHovered(false);
    if (stackPreview || pointerInteraction.current) {
      const active = document.activeElement;
      if (active instanceof HTMLElement && event.currentTarget.contains(active)) active.blur();
      setFocused(false);
    }
  }}>
    <div className={`preview ${isPinned ? "pinned-preview" : ""}`} style={{ borderRadius: `${radius}px` }}>
      <img src={item.dataUrl} draggable={false} onError={() => console.error("Screenshot image failed to decode")}/>
      {!isPinned && <button className="floating-action pin-action" title="Pin to screen" onClick={(event) => { event.stopPropagation(); void performAction("pin",item).catch((error) => console.error("Pin failed", error)); }}><Pin size={15}/></button>}
      <button className="floating-action annotate-action" title="Open annotation tool" onClick={() => performAction("annotate",item)}><Pencil size={15}/></button>
      <button className="close" title="Close" onClick={animatedClose}><X size={15}/></button>
      <button className="floating-action save-cloud-action" title="Save" onClick={() => void runAction("save")}>{completedAction === "save" ? <Check size={15}/> : <Download size={15}/>}</button>
      <div className="primary-actions">
        <button className={completedAction === "copy" ? "is-complete" : ""} onClick={() => void runAction("copy")}>{completedAction === "copy" ? <Check size={14}/> : "Copy"}</button>
        <button className={completedAction === "save" ? "is-complete" : ""} onClick={() => void runAction("save")}>{completedAction === "save" ? <Check size={14}/> : "Save"}</button>
      </div>
      <button className="drag-handle" title="Drag to another app"><GripHorizontal size={17}/></button>
      {isPinned && <div className="gesture-hud"><span>Pinch to resize</span><i/> <span>{opacity}% opacity</span></div>}
    </div>
  </div>
}
export default App;
