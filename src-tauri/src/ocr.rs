//! Local QR decoding and OCR. Network access is restricted to explicit model installation.
use std::{fs, io::{Read, Write}, path::{Path, PathBuf}, sync::Mutex, time::Duration};
use ocrs::{ImageSource, OcrEngine, OcrEngineParams};
use rten::Model;

const MODELS: [&str; 2] = ["text-detection.onnx", "text-recognition.onnx"];
const MODEL_BASE: &str = "https://ocrs-models.s3-accelerate.amazonaws.com";
const MAX_MODEL_BYTES: u64 = 128 * 1024 * 1024;

#[derive(Default)]
pub struct OcrState(Mutex<Option<OcrEngine>>);

pub fn ready(dir: &Path) -> bool {
    MODELS.iter().all(|name| fs::metadata(dir.join(name)).map(|m| m.is_file() && m.len() > 0).unwrap_or(false))
}

fn load(dir: &Path) -> Result<OcrEngine, String> {
    let detection = Model::load_file(dir.join(MODELS[0])).map_err(|e| format!("Invalid OCR detection model: {e}"))?;
    let recognition = Model::load_file(dir.join(MODELS[1])).map_err(|e| format!("Invalid OCR recognition model: {e}"))?;
    OcrEngine::new(OcrEngineParams {
        detection_model: Some(detection), recognition_model: Some(recognition), ..Default::default()
    }).map_err(|e| format!("Unable to initialize OCR: {e}"))
}

impl OcrState {
    pub fn install(&self, dir: &Path) -> Result<(), String> {
        // Serialize installation and inference; partial downloads are never used.
        let mut engine = self.0.lock().map_err(|_| "OCR engine unavailable")?;
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let client = reqwest::blocking::Client::builder()
            .connect_timeout(Duration::from_secs(20)).timeout(Duration::from_secs(300))
            .build().map_err(|e| e.to_string())?;
        for name in MODELS {
            let target = dir.join(name);
            let partial = dir.join(format!("{name}.download.onnx"));
            let result = (|| -> Result<(), String> {
                let response = client.get(format!("{MODEL_BASE}/{name}"))
                    .send().and_then(|r| r.error_for_status()).map_err(|e| format!("Model download failed: {e}"))?;
                let mut file = fs::File::create(&partial).map_err(|e| e.to_string())?;
                let bytes = std::io::copy(&mut response.take(MAX_MODEL_BYTES + 1), &mut file).map_err(|e| e.to_string())?;
                if bytes == 0 || bytes > MAX_MODEL_BYTES { return Err("Invalid OCR model download size".into()); }
                file.flush().map_err(|e| e.to_string())?;
                drop(file);
                Model::load_file(&partial).map_err(|e| format!("Downloaded OCR model is invalid: {e}"))?;
                // Windows rename does not replace an existing destination.
                if target.exists() { fs::remove_file(&target).map_err(|e| e.to_string())?; }
                fs::rename(&partial, &target).map_err(|e| e.to_string())?;
                Ok(())
            })();
            if result.is_err() { let _ = fs::remove_file(&partial); }
            result?;
        }
        *engine = Some(load(dir)?);
        Ok(())
    }

    pub fn recognize(&self, dir: &Path, path: PathBuf) -> Result<String, String> {
        let image = image::open(path).map_err(|e| format!("Unable to open screenshot: {e}"))?;
        // QR decoding needs neither the OCR models nor the inference lock.
        // Prefer exact decoded payloads over OCR guesses of surrounding text.
        if let Some(text) = crate::qr::decode(&image.to_luma8())? { return Ok(text); }
        if !ready(dir) {
            return Err("No readable QR code found. To recognize text, download models in Settings → Screenshots → OCR.".into());
        }
        let mut engine = self.0.try_lock().map_err(|_| "OCR is busy. Wait for the current recognition or model download to finish.")?;
        let total = std::time::Instant::now();
        let cold = engine.is_none();
        let stage = std::time::Instant::now();
        if engine.is_none() {
            if !ready(dir) { return Err("OCR models are not installed. Download models in the OCR panel first.".into()); }
            *engine = Some(load(dir)?);
        }
        let load_ms = stage.elapsed().as_secs_f64() * 1000.0;
        let stage = std::time::Instant::now();
        let image = image.into_rgb8();
        let decode_ms = stage.elapsed().as_secs_f64() * 1000.0;
        let stage = std::time::Instant::now();
        let source = ImageSource::from_bytes(image.as_raw(), image.dimensions()).map_err(|e| e.to_string())?;
        let engine = engine.as_ref().ok_or("OCR engine unavailable")?;
        let input = engine.prepare_input(source).map_err(|e| e.to_string())?;
        let prepare_ms = stage.elapsed().as_secs_f64() * 1000.0;
        let stage = std::time::Instant::now();
        let words = engine.detect_words(&input).map_err(|e| format!("Text detection failed: {e}"))?;
        let detect_ms = stage.elapsed().as_secs_f64() * 1000.0;
        let stage = std::time::Instant::now();
        let lines = engine.find_text_lines(&input, &words);
        let layout_ms = stage.elapsed().as_secs_f64() * 1000.0;
        let stage = std::time::Instant::now();
        let texts = engine.recognize_text(&input, &lines).map_err(|e| format!("Text recognition failed: {e}"))?;
        let recognize_ms = stage.elapsed().as_secs_f64() * 1000.0;
        // Match get_text's line joining; diagnostics never log screenshot text.
        let text = texts.iter().flatten().map(|line| line.to_string()).collect::<Vec<_>>().join("\n");
        if cfg!(debug_assertions) { eprintln!("OCR timing: cold={cold} size={}x{} load={load_ms:.1}ms decode={decode_ms:.1}ms prepare={prepare_ms:.1}ms detect={detect_ms:.1}ms layout={layout_ms:.1}ms recognize={recognize_ms:.1}ms total={:.1}ms", image.width(), image.height(), total.elapsed().as_secs_f64() * 1000.0); }
        Ok(text.trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn staging_models_keep_onnx_extension() {
        for name in MODELS {
            assert_eq!(Path::new(&format!("{name}.download.onnx")).extension().unwrap(), "onnx");
        }
    }

    #[test]
    fn missing_models_are_not_ready() {
        assert!(!ready(Path::new("/nonexistent/pureshot-ocr-models")));
    }
}
