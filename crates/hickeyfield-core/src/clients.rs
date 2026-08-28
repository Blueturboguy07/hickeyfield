//! Provider HTTP clients.
//!
//! Every request is made from this process with a credential read at assembly
//! time. There is no proxy and no server-side key — being native is what makes
//! that possible, and it is why providers that refuse browser requests
//! (Replicate, Topaz, first-party Kling) are reachable here at all.
//!
//! Polling is blocking rather than async on purpose. Providers cap concurrency
//! hard — a new fal account allows *two* simultaneous requests — so the thread
//! count is bounded by the provider long before it is bounded by us, and
//! blocking I/O keeps the trait object-safe without `async_trait`.

use crate::engine::{JobError, Output, OutputKind, PollResult, ProviderClient, Submission};
use crate::job::JobStatus;
use crate::media::Uploader;
use serde_json::Value;
use std::time::Duration;

/// Shared HTTP setup. 60s covers a slow submit; polling is separate and short.
fn http() -> Result<reqwest::blocking::Client, JobError> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(60))
        .user_agent(concat!("hickeyfield/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| JobError::Permanent(format!("could not build HTTP client: {e}")))
}

/// Read a JSON body, mapping transport failures to retryable errors and HTTP
/// status codes to the right kind.
fn json_or_err(resp: reqwest::blocking::Response) -> Result<Value, JobError> {
    let status = resp.status();
    let body = resp
        .text()
        .map_err(|e| JobError::Transient(format!("could not read response: {e}")))?;
    if !status.is_success() {
        // Include the body: provider errors are usually specific and useful,
        // and hiding them turns a two-minute fix into a support thread.
        return Err(JobError::from_status(status.as_u16(), body.trim()));
    }
    serde_json::from_str(&body)
        .map_err(|e| JobError::Permanent(format!("malformed provider response: {e}")))
}

/// An error together with what actually caused it.
///
/// `reqwest`'s Display stops at "error sending request for url (…)" — the DNS
/// failure, the TLS refusal, the reset connection are all one level down in the
/// source chain. Printing only the top line makes every network failure look
/// the same and none of them diagnosable.
fn with_cause(e: &dyn std::error::Error) -> String {
    let mut out = e.to_string();
    let mut src = e.source();
    while let Some(cause) = src {
        let text = cause.to_string();
        // Chains repeat themselves often enough that this is worth checking.
        if !out.contains(&text) {
            out.push_str(": ");
            out.push_str(&text);
        }
        src = cause.source();
    }
    out
}

fn send(req: reqwest::blocking::RequestBuilder) -> Result<Value, JobError> {
    match req.send() {
        Ok(r) => json_or_err(r),
        // A connection failure is worth retrying; a malformed URL is not, but
        // reqwest does not distinguish cleanly, and retrying a bad URL costs
        // four wasted seconds against losing a job to a flaky network.
        Err(e) => Err(JobError::Transient(format!("request failed: {e}"))),
    }
}

/// Collect output URLs from the shapes providers actually return. They differ
/// enough that guessing one canonical key would drop results silently.
fn outputs_from(v: &Value) -> Vec<Output> {
    let mut out = Vec::new();
    let mut push = |url: &str, kind: OutputKind| {
        if !url.is_empty() && !out.iter().any(|o: &Output| o.url == url) {
            out.push(Output {
                url: url.to_string(),
                kind,
                local_path: None,
            });
        }
    };

    // { "video": { "url": ... } } and { "audio": { "url": ... } }
    for (key, kind) in [
        ("video", OutputKind::Video),
        ("audio", OutputKind::Audio),
        ("image", OutputKind::Image),
    ] {
        if let Some(u) = v
            .get(key)
            .and_then(|x| x.get("url"))
            .and_then(Value::as_str)
        {
            push(u, kind);
        }
    }

    // { "images": [ { "url": ... } ] } and { "videos": [...] }
    for (key, kind) in [
        ("images", OutputKind::Image),
        ("videos", OutputKind::Video),
        ("audios", OutputKind::Audio),
    ] {
        if let Some(arr) = v.get(key).and_then(Value::as_array) {
            for item in arr {
                let u = item
                    .get("url")
                    .and_then(Value::as_str)
                    .or_else(|| item.as_str());
                if let Some(u) = u {
                    push(u, kind);
                }
            }
        }
    }

    out
}

/// Normalize a provider's status string onto our enum.
///
/// The vocabulary is Higgsfield's, because their intermediate stages
/// (`dna`, `script`, `visuals`) drive the multi-step progress copy in the
/// studio surfaces. Unknown strings map to `InProgress` rather than failing —
/// a provider inventing a new stage name should not kill a running job.
pub fn normalize_status(raw: &str) -> JobStatus {
    match raw.to_ascii_lowercase().as_str() {
        "pending" => JobStatus::Pending,
        "waiting" | "in_queue" => JobStatus::Waiting,
        "queued" => JobStatus::Queued,
        "dna" => JobStatus::Dna,
        "script" => JobStatus::Script,
        "visuals" => JobStatus::Visuals,
        "vision" => JobStatus::Vision,
        "flow" => JobStatus::Flow,
        "ip_detect" => JobStatus::IpDetect,
        "completed" | "complete" | "succeeded" | "success" | "ok" => JobStatus::Completed,
        "failed" | "error" => JobStatus::Failed,
        "nsfw" | "content_moderated" => JobStatus::Nsfw,
        "ip_detected" => JobStatus::IpDetected,
        "canceled" | "cancelled" => JobStatus::Canceled,
        _ => JobStatus::InProgress,
    }
}

// ---------------------------------------------------------------------------
// fal.ai — the backbone
// ---------------------------------------------------------------------------

/// Status and result URLs for a fal request.
///
/// `poll_ref` is fal's own `status_url` when we captured one at submit, and a
/// model path otherwise. Preferring theirs is not fastidiousness: a submit to
/// `fal-ai/bytedance/seedance-2.0/mini/text-to-video` is polled under
/// `fal-ai/bytedance`, so every URL we build ourselves — from the submit path
/// or from the family root — answers 405. The fallback exists only for rows
/// written before we started recording it.
fn fal_urls(poll_ref: &str, request_id: &str) -> (String, String) {
    if poll_ref.starts_with("http") {
        let base = poll_ref.trim_end_matches("/status").to_string();
        return (format!("{base}/status"), base);
    }
    let base = format!("https://queue.fal.run/{poll_ref}/requests/{request_id}");
    (format!("{base}/status"), base)
}

/// fal's queue API. Higgsfield's internal model ids are byte-for-byte fal slugs
/// minus the `fal-ai/` prefix, so their request shapes and ours line up almost
/// exactly — the single biggest saving in the whole build.
pub struct FalClient {
    key: String,
}

impl FalClient {
    pub fn new(key: impl Into<String>) -> Self {
        FalClient { key: key.into() }
    }

    fn auth(&self) -> String {
        format!("Key {}", self.key)
    }
}

impl ProviderClient for FalClient {
    fn submit(&self, model_id: &str, body: &Value) -> Result<Submission, JobError> {
        let v = send(
            http()?
                .post(format!("https://queue.fal.run/{model_id}"))
                .header("Authorization", self.auth())
                .json(body),
        )?;
        let request_id = v
            .get("request_id")
            .and_then(Value::as_str)
            .map(String::from)
            .ok_or_else(|| JobError::Permanent(format!("fal returned no request_id: {v}")))?;
        Ok(Submission {
            request_id,
            // Keep fal's own URL. It is scoped to a prefix that is neither the
            // path we posted to nor the family root, so anything we derive is
            // a guess that answers 405.
            status_url: v
                .get("status_url")
                .and_then(Value::as_str)
                .map(String::from),
        })
    }

    fn poll(&self, poll_ref: &str, request_id: &str) -> Result<PollResult, JobError> {
        let (status_url, result_url) = fal_urls(poll_ref, request_id);
        let v = send(
            http()?
                .get(&status_url)
                .header("Authorization", self.auth()),
        )?;

        let raw = v.get("status").and_then(Value::as_str).unwrap_or("");
        let status = normalize_status(raw);

        let mut outputs = Vec::new();
        if status == JobStatus::Completed {
            let r = send(
                http()?
                    .get(&result_url)
                    .header("Authorization", self.auth()),
            )?;
            outputs = outputs_from(&r);
        }

        Ok(PollResult {
            status,
            outputs,
            fail_reason: v.get("error").and_then(Value::as_str).map(String::from),
            actual_usd: None,
        })
    }

    fn cancel(&self, poll_ref: &str, request_id: &str) -> Result<(), JobError> {
        let (status_url, _) = fal_urls(poll_ref, request_id);
        let cancel_url = status_url.replace("/status", "/cancel");
        send(
            http()?
                .put(&cancel_url)
                .header("Authorization", self.auth()),
        )
        .map(|_| ())
    }

    fn poll_interval(&self) -> Duration {
        // fal's own SDK polls at 500ms.
        Duration::from_millis(500)
    }
}

// ---------------------------------------------------------------------------
// Higgsfield's public API — the only literal-Soul and literal-DoP path
// ---------------------------------------------------------------------------

/// `platform.higgsfield.ai`.
///
/// Two things make this one different. Auth is `Key {key}:{secret}` — a pair,
/// and not Bearer. And results are deleted after **7 days**, so a job here is
/// not finished until the bytes are on disk.
///
/// This provider is deliberately unadvertised: it lets a user reach Soul and
/// DoP with their own Higgsfield key, which is the one gap we cannot otherwise
/// close, but it is their key and their contract, not a headline feature.
pub struct HiggsfieldClient {
    key: String,
    secret: String,
}

impl HiggsfieldClient {
    pub fn new(key: impl Into<String>, secret: impl Into<String>) -> Self {
        HiggsfieldClient {
            key: key.into(),
            secret: secret.into(),
        }
    }

    fn auth(&self) -> String {
        format!("Key {}:{}", self.key, self.secret)
    }
}

impl ProviderClient for HiggsfieldClient {
    fn submit(&self, model_id: &str, body: &Value) -> Result<Submission, JobError> {
        let v = send(
            http()?
                .post(format!("https://platform.higgsfield.ai/{model_id}"))
                .header("Authorization", self.auth())
                .header("Accept", "application/json")
                .json(body),
        )
        // `model_not_found` is not a typo in our slug: the API-key platform
        // serves only the ~50 fal-shaped paths in its OpenAPI spec (verified
        // 2026-08-13), and the Studio compilers, Soul compositions, 3D family
        // and explainers exist only behind their logged-in product gateway
        // (fnf-api-gw, Clerk OAuth) — no key pair reaches them. Saying that is
        // more useful than relaying the raw 404.
        .map_err(|e| match &e {
            JobError::Permanent(msg) | JobError::Transient(msg)
                if msg.contains("model_not_found") =>
            {
                JobError::Permanent(format!(
                    "Higgsfield's API platform does not serve `{model_id}` — an API key only \
                     reaches their fal-style model paths, and this surface exists only inside \
                     their logged-in web app. Pick a different model for this job; for editing \
                     a clip, Grok Edit Video, Ray2 Modify and Wan VACE run via fal."
                ))
            }
            _ => e,
        })?;
        let request_id = v
            .get("request_id")
            .and_then(Value::as_str)
            .map(String::from)
            .ok_or_else(|| JobError::Permanent(format!("no request_id in response: {v}")))?;
        Ok(Submission {
            request_id,
            // Higgsfield's queue is flat, so there is nothing to remember.
            status_url: None,
        })
    }

    fn poll(&self, _poll_ref: &str, request_id: &str) -> Result<PollResult, JobError> {
        // Higgsfield's queue is flat, not app-scoped, so the model id is unused
        // here. Kept in the signature because fal needs it and one trait serves
        // both.
        let v = send(
            http()?
                .get(format!(
                    "https://platform.higgsfield.ai/requests/{request_id}/status"
                ))
                .header("Authorization", self.auth()),
        )?;
        let status = normalize_status(v.get("status").and_then(Value::as_str).unwrap_or(""));
        Ok(PollResult {
            status,
            outputs: outputs_from(&v),
            fail_reason: v.get("error").and_then(Value::as_str).map(String::from),
            actual_usd: None,
        })
    }

    fn cancel(&self, _poll_ref: &str, request_id: &str) -> Result<(), JobError> {
        // Documented as 202 on success, 400 otherwise, and only while queued.
        send(
            http()?
                .post(format!(
                    "https://platform.higgsfield.ai/requests/{request_id}/cancel"
                ))
                .header("Authorization", self.auth()),
        )
        .map(|_| ())
    }
}

// ---------------------------------------------------------------------------
// Local — auto-detected ComfyUI / Ollama
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Uploads — turning a local file into something a provider can fetch
// ---------------------------------------------------------------------------

/// fal's REST host. Confirmed against both official clients on 2026-08-04:
/// `fal-js` `libs/client/src/config.ts` and `fal_client/client.py` agree on
/// `https://rest.fal.ai`. An older `rest.alpha.fal.ai` alias still appears in
/// third-party writeups; it is not what the current clients use.
const FAL_REST: &str = "https://rest.fal.ai";

/// fal switches to a multipart flow above this size. We do not implement
/// multipart, so this is the point at which we must say so plainly rather than
/// send a request that fails halfway through a large video.
const FAL_MULTIPART_THRESHOLD: u64 = 90 * 1024 * 1024;

/// Guess a MIME type from the extension.
///
/// The upload is a signed PUT, so the `Content-Type` we declare at initiate is
/// the one the CDN will serve the file with. Getting it wrong makes the
/// provider reject a perfectly good image, so an unknown extension gets the
/// generic type rather than a confident wrong one.
fn mime_for(path: &str) -> &'static str {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "heic" => "image/heic",
        "mp4" | "m4v" => "video/mp4",
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "m4a" => "audio/mp4",
        "flac" => "audio/flac",
        "ogg" => "audio/ogg",
        _ => "application/octet-stream",
    }
}

fn file_name_of(path: &str) -> String {
    path.rsplit(['/', '\\'])
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("upload")
        .to_string()
}

/// The name to hand a storage service, reduced to characters that survive a
/// round trip through a URL path and a signature.
///
/// The provider echoes this name into the object path, and what it does with a
/// character outside ASCII is its business, not ours — fal replaced one with
/// `?`, which then read as the start of a query string and broke the upload of
/// a file whose only sin was being a macOS screenshot.
///
/// That character is worth naming, because it is invisible and extremely
/// common: macOS puts a **narrow no-break space** (U+202F) before the AM/PM in
/// every screenshot filename. `Screenshot 2026-08-06 at 3.52.23 PM.png` looks
/// like plain ASCII in every UI that displays it and is not.
///
/// The local file keeps its real name; this is only what we tell the provider
/// to call its copy. We use the returned URL and never the name, so there is
/// nothing to lose by making it boring.
fn safe_upload_name(path: &str) -> String {
    let raw = file_name_of(path);
    // Split on the last dot so the extension survives: fal sniffs content type
    // from it, and an upload that arrives as `application/octet-stream` is
    // rejected by some endpoints for a reason that names neither the file nor
    // the extension.
    let (stem, ext) = match raw.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() && e.chars().all(|c| c.is_ascii_alphanumeric()) => (s, e),
        _ => (raw.as_str(), ""),
    };

    let mut out = String::with_capacity(stem.len());
    let mut last_was_fill = false;
    for c in stem.chars() {
        if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
            out.push(c);
            last_was_fill = false;
        } else if !last_was_fill {
            // One underscore per run, so a name of spaces does not become a
            // name of underscores.
            out.push('_');
            last_was_fill = true;
        }
    }
    // Long names are their own failure mode on object stores with a path limit.
    let trimmed: String = out.trim_matches('_').chars().take(80).collect();
    let stem = if trimmed.is_empty() {
        "upload"
    } else {
        &trimmed
    };

    if ext.is_empty() {
        stem.to_string()
    } else {
        format!("{stem}.{}", ext.to_ascii_lowercase())
    }
}

/// Reject sizes the single-shot upload cannot carry, before any bytes move.
///
/// Split out from [`read_upload`] so the limit can be tested without writing a
/// 90 MB file to disk.
fn size_refusal(len: u64, name: &str) -> Option<String> {
    if len == 0 {
        return Some(format!("{name} is empty"));
    }
    if len > FAL_MULTIPART_THRESHOLD {
        // Better to refuse clearly than to start a 90 MB upload that the
        // endpoint will not accept whole — a mid-transfer failure reads as a
        // network problem rather than as a limit.
        return Some(format!(
            "{name} is {:.0} MB — files above 90 MB need fal's multipart upload, which Hickeyfield does not implement yet. Trim the clip or upload it to a URL first.",
            len as f64 / 1_048_576.0
        ));
    }
    None
}

/// Read a local file, refusing early on the cases that would fail confusingly.
fn read_upload(path: &str) -> Result<Vec<u8>, String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("could not read {path}: {e}"))?;
    if let Some(why) = size_refusal(meta.len(), &file_name_of(path)) {
        return Err(why);
    }
    std::fs::read(path).map_err(|e| format!("could not read {path}: {e}"))
}

impl Uploader for FalClient {
    /// Two-step signed upload, matching both official clients:
    /// `POST /storage/upload/initiate {file_name, content_type}` →
    /// `{upload_url, file_url}`, then `PUT upload_url` with the raw bytes.
    ///
    /// The returned `file_url` is public and free, which is what makes fal the
    /// upload path for providers that only accept URLs — notably Kling and
    /// Alibaba.
    fn upload(&self, path: &str) -> Result<String, String> {
        let bytes = read_upload(path)?;
        let content_type = mime_for(path);

        let init = send(
            http()
                .map_err(|e| e.to_string())?
                // `storage_type` matches the current fal-js default. The Python
                // client passes `gcs`; both are accepted, and pinning one keeps
                // the returned host stable across a session.
                .post(format!(
                    "{FAL_REST}/storage/upload/initiate?storage_type=fal-cdn-v3"
                ))
                .header("Authorization", self.auth())
                .json(&serde_json::json!({
                    "file_name": safe_upload_name(path),
                    "content_type": content_type,
                })),
        )
        .map_err(|e| format!("fal would not accept the upload: {e}"))?;

        let upload_url = init
            .get("upload_url")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("fal returned no upload_url: {init}"))?;
        let file_url = init
            .get("file_url")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("fal returned no file_url: {init}"))?
            .to_string();

        // The PUT is to a pre-signed URL and must NOT carry our key.
        let resp = http()
            .map_err(|e| e.to_string())?
            .put(upload_url)
            .header("Content-Type", content_type)
            .body(bytes)
            .send()
            // reqwest's own message is "error sending request for url (…)",
            // which names the URL and nothing about why — the cause lives in
            // the source chain, and dropping it left a transport failure
            // indistinguishable from a rejection.
            .map_err(|e| format!("upload failed: {}", with_cause(&e)))?;
        if !resp.status().is_success() {
            return Err(format!(
                "upload rejected with HTTP {}",
                resp.status().as_u16()
            ));
        }

        Ok(file_url)
    }
}

impl Uploader for HiggsfieldClient {
    /// `POST /files/generate-upload-url {content_type}` → `{public_url, upload_url,
    /// upload_headers}`, then `PUT upload_url`. Verified against their published
    /// API surface.
    ///
    /// Note the response field is `public_url`, not fal's `file_url`; reading
    /// the wrong one yields `None` and an upload that appears to succeed while
    /// producing nothing usable.
    fn upload(&self, path: &str) -> Result<String, String> {
        let bytes = read_upload(path)?;
        let content_type = mime_for(path);

        let init = send(
            http()
                .map_err(|e| e.to_string())?
                .post("https://platform.higgsfield.ai/files/generate-upload-url")
                .header("Authorization", self.auth())
                .json(&serde_json::json!({ "content_type": content_type })),
        )
        .map_err(|e| format!("Higgsfield would not accept the upload: {e}"))?;

        let upload_url = init
            .get("upload_url")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("Higgsfield returned no upload_url: {init}"))?;
        let public_url = init
            .get("public_url")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("Higgsfield returned no public_url: {init}"))?
            .to_string();

        // The presigned URL is signed over every header in `upload_headers` —
        // Content-Type plus `x-amz-tagging: retention=temporary` today. A PUT
        // that carries only Content-Type fails S3's signature check, which
        // surfaces as an opaque 403.
        let mut req = http().map_err(|e| e.to_string())?.put(upload_url);
        match init.get("upload_headers").and_then(Value::as_object) {
            Some(headers) => {
                for (name, value) in headers {
                    if let Some(v) = value.as_str() {
                        req = req.header(name.as_str(), v);
                    }
                }
            }
            None => req = req.header("Content-Type", content_type),
        }
        let resp = req
            .body(bytes)
            .send()
            .map_err(|e| format!("upload failed: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!(
                "upload rejected with HTTP {}",
                resp.status().as_u16()
            ));
        }

        Ok(public_url)
    }
}

pub const COMFY_URL: &str = "http://127.0.0.1:8188";
pub const OLLAMA_URL: &str = "http://127.0.0.1:11434";

/// What, if anything, is running locally. Costs nothing to use and needs no
/// key — the genuinely free tier, and only possible because we are native.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize)]
pub struct LocalEndpoints {
    pub comfyui: bool,
    pub ollama: bool,
}

impl LocalEndpoints {
    pub fn any(&self) -> bool {
        self.comfyui || self.ollama
    }
}

/// Probe for local inference. Short timeout: this runs at startup and must
/// never delay the window appearing.
pub fn detect_local() -> LocalEndpoints {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_millis(400))
        .build();
    let Ok(client) = client else {
        return LocalEndpoints::default();
    };
    let up = |url: String| {
        client
            .get(url)
            .send()
            .map(|r| r.status().is_success())
            .unwrap_or(false)
    };
    LocalEndpoints {
        comfyui: up(format!("{COMFY_URL}/system_stats")),
        ollama: up(format!("{OLLAMA_URL}/api/tags")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_vocabulary_covers_higgsfields_stages() {
        // The intermediate stages are not noise — they drive the studio
        // progress labels ("Extracting product DNA", "Writing a script").
        assert_eq!(normalize_status("dna"), JobStatus::Dna);
        assert_eq!(normalize_status("script"), JobStatus::Script);
        assert_eq!(normalize_status("visuals"), JobStatus::Visuals);
        assert_eq!(normalize_status("ip_detect"), JobStatus::IpDetect);
    }

    #[test]
    fn provider_synonyms_normalize_to_one_status() {
        for s in ["completed", "COMPLETE", "succeeded", "success", "ok"] {
            assert_eq!(normalize_status(s), JobStatus::Completed, "{s}");
        }
        for s in ["canceled", "cancelled"] {
            assert_eq!(normalize_status(s), JobStatus::Canceled, "{s}");
        }
        assert_eq!(normalize_status("content_moderated"), JobStatus::Nsfw);
    }

    #[test]
    fn an_unknown_status_keeps_the_job_alive() {
        // A provider inventing a stage name must not terminate a running job.
        let s = normalize_status("rendering_frames");
        assert_eq!(s, JobStatus::InProgress);
        assert!(!s.is_terminal());
    }

    #[test]
    fn collects_outputs_from_every_shape_providers_use() {
        let single = serde_json::json!({ "video": { "url": "https://a/v.mp4" } });
        assert_eq!(outputs_from(&single).len(), 1);
        assert_eq!(outputs_from(&single)[0].kind, OutputKind::Video);

        let list = serde_json::json!({ "images": [
            { "url": "https://a/1.png" }, { "url": "https://a/2.png" }
        ]});
        assert_eq!(outputs_from(&list).len(), 2);
        assert_eq!(outputs_from(&list)[0].kind, OutputKind::Image);

        let bare = serde_json::json!({ "images": ["https://a/3.png"] });
        assert_eq!(outputs_from(&bare).len(), 1);
    }

    #[test]
    fn duplicate_urls_are_collapsed() {
        let v = serde_json::json!({
            "video": { "url": "https://a/v.mp4" },
            "videos": [{ "url": "https://a/v.mp4" }]
        });
        assert_eq!(outputs_from(&v).len(), 1, "same URL reported twice");
    }

    #[test]
    fn no_outputs_is_empty_not_an_error() {
        assert!(outputs_from(&serde_json::json!({ "status": "queued" })).is_empty());
    }

    #[test]
    fn higgsfield_auth_is_a_key_secret_pair_not_bearer() {
        // Getting this wrong yields a 401 that looks like a bad key.
        let c = HiggsfieldClient::new("k1", "s1");
        assert_eq!(c.auth(), "Key k1:s1");
        assert!(!c.auth().starts_with("Bearer"));
    }

    #[test]
    fn fal_polls_faster_than_the_generic_default() {
        assert_eq!(
            FalClient::new("k").poll_interval(),
            Duration::from_millis(500)
        );
        assert_eq!(
            HiggsfieldClient::new("k", "s").poll_interval(),
            Duration::from_secs(3)
        );
    }

    #[test]
    fn local_endpoints_report_availability() {
        let none = LocalEndpoints::default();
        assert!(!none.any());
        assert!(LocalEndpoints {
            comfyui: true,
            ollama: false
        }
        .any());
    }

    // ── Uploads ────────────────────────────────────────────────────────────

    #[test]
    fn a_providers_own_status_url_is_used_verbatim() {
        // The 405 bug, twice over. A submit to
        // fal-ai/bytedance/seedance-2.0/mini/text-to-video is polled under
        // fal-ai/bytedance — neither the submit path nor the family root — so
        // any URL we build ourselves is wrong. fal tells us; believe it.
        let given = "https://queue.fal.run/fal-ai/bytedance/requests/abc/status";
        let (status, result) = fal_urls(given, "abc");
        assert_eq!(status, given);
        assert_eq!(
            result,
            "https://queue.fal.run/fal-ai/bytedance/requests/abc"
        );
    }

    #[test]
    fn a_status_url_without_the_suffix_still_yields_both() {
        // Providers are inconsistent about whether the stored URL ends in
        // /status; both shapes must give the same pair.
        let (status, result) = fal_urls("https://queue.fal.run/fal-ai/x/requests/abc", "abc");
        assert!(status.ends_with("/status"));
        assert!(!result.ends_with("/status"));
    }

    #[test]
    fn a_model_path_falls_back_to_the_constructed_url() {
        // Only for rows written before we recorded the real one.
        let (status, _) = fal_urls("fal-ai/nano-banana-2", "abc");
        assert_eq!(
            status,
            "https://queue.fal.run/fal-ai/nano-banana-2/requests/abc/status"
        );
    }

    #[test]
    fn mime_types_cover_what_the_media_roles_actually_accept() {
        // A wrong Content-Type makes the CDN serve the file as the wrong type
        // and the provider reject a perfectly good image.
        assert_eq!(mime_for("/a/b/frame.png"), "image/png");
        assert_eq!(mime_for("shot.JPEG"), "image/jpeg");
        assert_eq!(mime_for("clip.mov"), "video/quicktime");
        assert_eq!(mime_for("take.mp4"), "video/mp4");
        assert_eq!(mime_for("vo.wav"), "audio/wav");
    }

    #[test]
    fn an_unknown_extension_gets_the_generic_type_not_a_confident_guess() {
        assert_eq!(mime_for("noextension"), "application/octet-stream");
        assert_eq!(mime_for("archive.tar.zst"), "application/octet-stream");
    }

    #[test]
    fn file_names_survive_both_path_separators() {
        // The Windows build hands us backslashes; splitting on '/' alone would
        // upload the whole path as the file name.
        assert_eq!(file_name_of("/Users/x/Pictures/a.png"), "a.png");
        assert_eq!(file_name_of(r"C:\Users\x\Pictures\a.png"), "a.png");
        assert_eq!(file_name_of("bare.png"), "bare.png");
    }

    #[test]
    fn a_missing_file_names_the_path_rather_than_failing_opaquely() {
        let e = read_upload("/definitely/not/here.png").unwrap_err();
        assert!(e.contains("/definitely/not/here.png"), "got: {e}");
    }

    #[test]
    fn an_oversized_file_is_refused_before_any_bytes_move() {
        // fal switches to multipart above 90 MB and we do not implement it.
        // Starting the upload anyway fails most of the way through a large
        // video, which reads as a network problem rather than as a limit.
        let over = size_refusal(FAL_MULTIPART_THRESHOLD + 1, "take.mov").unwrap();
        assert!(over.contains("take.mov"), "got: {over}");
        assert!(over.contains("90 MB"), "got: {over}");
        // It must say what to do about it, not just that it failed.
        assert!(over.contains("Trim"), "got: {over}");
    }

    #[test]
    fn a_file_at_exactly_the_limit_is_allowed() {
        // An off-by-one here silently refuses a file the endpoint accepts.
        assert!(size_refusal(FAL_MULTIPART_THRESHOLD, "edge.mp4").is_none());
    }

    #[test]
    fn an_empty_file_is_refused_with_a_reason() {
        // Zero bytes uploads "successfully" and then produces a provider error
        // about a corrupt image, which sends the user looking in the wrong place.
        let why = size_refusal(0, "empty.png").unwrap();
        assert!(
            why.contains("empty.png") && why.contains("empty"),
            "got: {why}"
        );
    }

    #[test]
    fn an_ordinary_file_passes_the_guard() {
        assert!(size_refusal(2 * 1024 * 1024, "frame.png").is_none());
    }

    #[test]
    fn a_macos_screenshot_name_is_made_url_safe() {
        // The exact name that failed, U+202F and all. macOS puts a narrow
        // no-break space before AM/PM in every screenshot filename; fal turned
        // it into `?` inside the object path, which reads as the start of a
        // query string, and the upload died with a transport error naming a URL
        // that looked perfectly ordinary on screen.
        let name =
            safe_upload_name("/Users/x/Desktop/Screenshot 2026-08-06 at 3.52.23\u{202f}PM.png");
        assert_eq!(name, "Screenshot_2026-08-06_at_3_52_23_PM.png");
        assert!(
            name.is_ascii(),
            "a name we hand a storage service must survive a URL path"
        );
    }

    #[test]
    fn the_extension_survives_because_fal_sniffs_content_type_from_it() {
        assert_eq!(safe_upload_name("/tmp/clip.MP4"), "clip.mp4");
        assert_eq!(safe_upload_name("/tmp/no-extension"), "no-extension");
    }

    #[test]
    fn a_run_of_junk_collapses_rather_than_becoming_a_row_of_underscores() {
        // Hyphens are legal in a URL path and are kept verbatim; only the
        // characters being *replaced* collapse into a single underscore.
        assert_eq!(safe_upload_name("/tmp/a   b -- c.png"), "a_b_--_c.png");
    }

    #[test]
    fn a_name_with_nothing_usable_still_produces_a_name() {
        assert_eq!(safe_upload_name("/tmp/???.png"), "upload.png");
        assert_eq!(safe_upload_name("/tmp/\u{4e2d}\u{6587}.jpg"), "upload.jpg");
    }

    #[test]
    fn a_very_long_name_is_bounded() {
        let long = format!("/tmp/{}.png", "a".repeat(500));
        let out = safe_upload_name(&long);
        assert!(out.len() <= 84, "{} chars", out.len());
        assert!(out.ends_with(".png"));
    }

    #[test]
    fn a_dot_in_the_body_does_not_eat_the_extension() {
        assert_eq!(safe_upload_name("/tmp/v1.2.final.mov"), "v1_2_final.mov");
    }
}
