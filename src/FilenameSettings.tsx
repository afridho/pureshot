import { useEffect, useRef, useState } from "react";
import { invoke, isTauri } from "@tauri-apps/api/core";

const presets = [
  { label: "PureShot + date & time", value: "PureShot {date} {time}" },
  { label: "Compact date & time", value: "{year}{month}{day}_{hour}{minute}{second}" },
  { label: "Capture type + date & time", value: "PureShot {type} {date} {time}" },
  { label: "Date, time & dimensions", value: "PureShot {date} {time} {width}x{height}" },
  { label: "Timestamp", value: "PureShot {timestamp}" },
];
const variables = [
  ["date", "Date: YYYY-MM-DD"], ["time", "Time: HH-MM-SS"],
  ["year", "Four-digit year"], ["month", "Two-digit month"], ["day", "Two-digit day"],
  ["hour", "Hour (24-hour)"], ["minute", "Minute"], ["second", "Second"],
  ["timestamp", "Unix timestamp in milliseconds"], ["width", "Capture width in pixels"],
  ["height", "Capture height in pixels"], ["type", "Capture type: area, window, fullscreen"], ["id", "Unique capture ID"],
];

export default function FilenameSettings({ defaultFormat }: { defaultFormat: string }) {
  const [template, setTemplate] = useState(presets[0].value);
  const [saved, setSaved] = useState(presets[0].value);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [preview, setPreview] = useState("");
  const [previewBusy, setPreviewBusy] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [saveError, setSaveError] = useState<string | null>(null);
  const input = useRef<HTMLInputElement>(null);
  useEffect(() => {
    if (!isTauri()) { setLoading(false); return; }
    let active = true;
    void invoke<string>("get_filename_template").then(value => {
      if (active) { setTemplate(value); setSaved(value); }
    }).catch(error => { if (active) setSaveError(String(error)); })
      .finally(() => { if (active) setLoading(false); });
    return () => { active = false; };
  }, []);
  useEffect(() => {
    if (!isTauri()) return;
    let active = true;
    setPreviewBusy(true);
    setError(null);
    const timer = window.setTimeout(() => {
      void invoke<string>("preview_filename_template", { template }).then(value => {
        if (active) setPreview(value);
      }).catch(error => { if (active) { setError(String(error)); setPreview(""); } })
        .finally(() => { if (active) setPreviewBusy(false); });
    }, 200);
    return () => { active = false; window.clearTimeout(timer); };
  }, [template, defaultFormat]);
  const update = (value: string) => { setTemplate(value); setPreviewBusy(true); setSaveError(null); };
  const insert = (variable: string) => {
    const element = input.current;
    const start = element?.selectionStart ?? template.length;
    const end = element?.selectionEnd ?? start;
    const token = `{${variable}}`;
    update(template.slice(0, start) + token + template.slice(end));
    requestAnimationFrame(() => { element?.focus(); element?.setSelectionRange(start + token.length, start + token.length); });
  };
  const apply = async () => {
    setSaving(true); setSaveError(null);
    try { await invoke("set_filename_template", { template }); setSaved(template); }
    catch (error) { setSaveError(String(error)); }
    finally { setSaving(false); }
  };
  const disabled = loading || saving || !isTauri();
  return <div className="filename-settings">
    <div className="filename-heading"><label htmlFor="filename-preset">Filename template</label>
      <select id="filename-preset" value={presets.some(p => p.value === template) ? template : "custom"} disabled={disabled} onChange={event => { if (event.target.value !== "custom") update(event.target.value); }}>
        {presets.map(preset => <option key={preset.value} value={preset.value}>{preset.label}</option>)}
        <option value="custom">Custom</option>
      </select>
    </div>
    <div className="filename-input-row"><input ref={input} aria-label="Custom filename template" aria-describedby="filename-help filename-preview" aria-invalid={!!error} value={template} maxLength={120} disabled={disabled} onChange={event => update(event.target.value)}/>
      <button type="button" disabled={disabled || previewBusy || !!error || template === saved} onClick={() => void apply()}>{saving ? "Saving…" : "Apply"}</button></div>
    <p id="filename-help">Click a variable to insert it. Uses capture date/time in your local timezone; the default format extension is added automatically.</p>
    <div className="filename-variables">{variables.map(([variable, description]) => <button type="button" key={variable} title={description} aria-label={`Insert ${variable}: ${description}`} disabled={disabled} onClick={() => insert(variable)}>{`{${variable}}`}</button>)}</div>
    <p id="filename-preview" className="filename-preview" aria-live="polite">{!isTauri() ? "Filename settings are available in the desktop app." : error || saveError ? <span role="alert">{error ?? saveError}</span> : <>Preview: <code>{previewBusy ? "Updating…" : preview}</code>{template === saved && !loading && !previewBusy && <span> · Saved</span>}</>}</p>
    <p>Existing files are kept; duplicate names receive (2), (3), etc. Annotations add “Annotated”.</p>
  </div>;
}
