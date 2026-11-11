# Hickeyfield: notes for coding agents

## publik API
- Base URL: https://publikhq.com/api/v1 (OpenAI Chat Completions, Responses, Anthropic Messages)
- Models: publik-fast, publik-balanced, publik-smart only; never a vendor model name
- Sign in with publik: the publik module's gate runs at start-up, before the first screen; it also sends usage telemetry
- Key: per-install `pk_` from POST /installs, stored at the publik credential path; env PUBLIK_API_KEY / PUBLIK_API_BASE_URL override
- App token: publik-app-token.txt (public by design, committed) or PUBLIK_APP_TOKEN
- 402: show error.message plus error.top_up_url only
- Never print or log a key
- Docs: https://publikhq.com/llms.txt, https://publikhq.com/developers, https://publikhq.com/skills/api.md

## Where it lives in this repo
- `src-tauri/src/publik.rs`: the module (gate, usage counting, publik API calls). `APP_SLUG = "hickeyfield"`, `USES_AI = true`.
- `src-tauri/src/publik_gate.rs`: the gate's screens as native dialogs; `publik_gate::run` is the first call in the setup hook. The main window starts hidden and is shown after the gate.
- `src-tauri/build.rs`: compiles the app token in from `PUBLIK_APP_TOKEN` or `src-tauri/publik-app-token.txt`.
- `scripts/publik-self-test.py`: CI proof that the built app runs the gate (`PUBLIK_SELF_TEST=1`).
- Provider keys (fal, Higgsfield, ...) stay in the OS keychain (`src-tauri/src/vault.rs`): "Use my own key" changes nothing there.
