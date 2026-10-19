import { useCallback, useEffect, useState } from "react";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { CaptureItem } from "./types";
import "./ocr.css";

/** Shared, explicit opt-in model setup for Settings and OCR results. */
export function OcrSetup({ onReady, captureButton = false }: { onReady?: (ready: boolean) => void; captureButton?: boolean }) {
  const [ready, setReady] = useState<boolean | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [capturing, setCapturing] = useState(false);
  const check = useCallback(async () => {
    setError(null);
    try {
      const available = await invoke<boolean>("get_ocr_status");
      setReady(available);
      onReady?.(available);
    } catch (error) { setError(String(error)); }
  }, [onReady]);
  useEffect(() => { if (isTauri()) void check(); }, [check]);
  const install = async () => {
    setBusy(true); setError(null);
    try {
      await invoke("install_ocr_models");
      await check();
    } catch (error) { setError(String(error)); }
    finally { setBusy(false); }
  };
  const captureText = async () => {
    setCapturing(true); setError(null);
    try { await invoke("start_ocr_capture"); }
    catch (error) { if (String(error) !== "Screenshot cancelled") setError(String(error)); }
    finally { setCapturing(false); }
  };
  return <div className="ocr-setup ocr-controls">
    <strong>Text recognition (preview)</strong>
    <p role="status">{!isTauri() ? "Available in the desktop app only." : busy ? "Downloading OCR models… Please keep PureShot open." : ready === null ? "Checking OCR setup…" : ready ? "OCR models are installed. Recognition runs locally." : "OCR models are not installed."}</p>
    <p>QR codes work offline without downloading models. Capture the entire code, including its border. QR content is copied as text; links are never opened automatically. Download the OCR models (about 12 MB) for Latin-script text recognition when no QR code is found. Everything stays on your device.</p>
    <div className="ocr-actions">
      <button type="button" disabled={busy || !isTauri()} onClick={() => void install()}>{busy ? "Downloading…" : ready ? "Reinstall OCR models" : "Download OCR models"}</button>
      <button type="button" disabled={busy || !isTauri()} onClick={() => void check()}>Refresh status</button>
      {captureButton && <button type="button" disabled={!isTauri() || capturing} onClick={() => void captureText()}>{capturing ? "Selecting…" : "Capture OCR"}</button>}
    </div>
    {error && <p role="alert">{error}</p>}
  </div>;
}

export default function OcrView() {
  const [item, setItem] = useState<CaptureItem | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [ready, setReady] = useState<boolean | null>(null);
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);
  const [complete, setComplete] = useState(false);
  const [copying, setCopying] = useState(false);
  const [copyMessage, setCopyMessage] = useState("");
  const load = useCallback(async () => {
    setLoadError(null);
    try {
      const capture = await invoke<CaptureItem | null>("get_window_capture");
      if (!capture) throw new Error("This screenshot is no longer available. Open a capture from History or capture text again.");
      setItem(capture);
    } catch (error) { setLoadError(String(error)); }
  }, []);
  useEffect(() => { void load(); }, [load]);
  useEffect(() => {
    if (!item || ready === null) return;
    let active = true;
    setBusy(true); setError(null); setCopyMessage(""); setComplete(false);
    void invoke<string>("extract_capture_text", { id: item.id })
      .then(async result => {
        if (!active) return;
        setText(result); setComplete(true);
        if (result.trim()) {
          try {
            await invoke("copy_ocr_text", { text: result });
            if (active) setCopyMessage("Text copied to clipboard.");
          } catch (copyError) { if (active) setCopyMessage(`Copy failed: ${String(copyError)}`); }
        }
      })
      .catch(error => { if (active) setError(String(error)); })
      .finally(() => { if (active) setBusy(false); });
    return () => { active = false; };
  }, [item, ready, attempt]);
  const copy = async () => {
    setCopying(true); setCopyMessage("");
    try { await invoke("copy_ocr_text", { text }); setCopyMessage("Copied to clipboard."); }
    catch (error) { setCopyMessage(`Copy failed: ${String(error)}`); }
    finally { setCopying(false); }
  };
  return <main className="ocr-view ocr-controls">
    <header><div><h1>Extract Text</h1><p>QR codes are checked first; text recognition is used if no QR code is decoded.</p></div><button type="button" onClick={() => void getCurrentWindow().close()}>Close</button></header>
    <OcrSetup onReady={setReady}/>
    {loadError ? <div><p role="alert">{loadError}</p><button type="button" onClick={() => void load()}>Retry loading screenshot</button></div> : !item ? <p role="status">Loading screenshot…</p> : <div className="ocr-result">
      <figure><img src={item.dataUrl} alt="Screenshot used for text recognition"/><figcaption>{item.width} × {item.height}</figcaption></figure>
      <section aria-busy={busy}>
        <label htmlFor="ocr-text">Recognized text or QR content (editable)</label>
        <textarea id="ocr-text" value={text} readOnly={busy} spellCheck={false} placeholder={busy ? "Recognizing text…" : "Recognized text will appear here. You can also type or correct text."} onChange={event => { setText(event.target.value); setCopyMessage(""); }}/>
        <p role="status">{busy ? "Reading QR codes and text locally…" : complete && !text.trim() ? "No text or QR code found. Try a clearer or larger capture, or enter text manually." : ""}</p>
        {error && <p role="alert">Recognition failed: {error}</p>}
        <div className="ocr-actions"><button type="button" disabled={busy} onClick={() => setAttempt(value => value + 1)}>Retry recognition</button><button type="button" disabled={busy || copying || !text.trim()} onClick={() => void copy()}>{copying ? "Copying…" : "Copy text"}</button></div>
        <p role="status">{copyMessage}</p>
      </section>
    </div>}
  </main>;
}
