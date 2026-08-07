import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  cancelJob,
  configuredProviders,
  estimateCost,
  isDesktop,
  keyStates,
  libraryRoot,
  listJobs,
  listModels,
  listPresets,
  listUseCases,
  modelsForUseCase,
  localEndpoints,
  providerInfo,
  submitJob,
  modelCapabilities,
  deleteJob,
  subscribeJobs,
  type KeyState,
} from "./api";
import type {
  CostEstimate,
  GenSettings,
  JobSet,
  MediaRef,
  MediaRole,
  Model,
  ModelCapabilities,
  PresetFamily,
  UseCase,
  WorkspaceTab,
} from "./types";
import { capabilitiesFor, resolveSettings, selectRoute } from "./lib/variants";
import { measureAspect, nearestAspect } from "./lib/media";
import { previewOf } from "./lib/media-input";
import { isRunning } from "./lib/status";
import {
  fromFileList,
  mergeMedia,
  pickViaDialog,
  sourceKey,
} from "./lib/media-input";
import {
  PROVIDER_CATALOG,
  isAppConfigured,
  type LocalEndpoints,
  type ProviderInfo,
} from "./lib/providers";
import { SettingsRail } from "./components/SettingsRail";
import { ResultsFeed } from "./components/ResultsFeed";
import { MetaRail } from "./components/MetaRail";
import { PresetPicker } from "./components/PresetPicker";
import { ModelPicker } from "./components/ModelPicker";
import { Onboarding } from "./components/Onboarding";
import { Settings } from "./components/Settings";
import { ConfirmDelete } from "./components/ConfirmDelete";
import { SettingsIcon } from "./components/Icons";
import "./app.css";

const DEFAULT_SETTINGS: GenSettings = {
  duration: 5,
  resolution: "1080p",
  aspect: "16:9",
  audio: false,
  // Enhance ships on. Raw prompts underperform on every hosted video model,
  // and the enhancer that ran is recorded per job so the rewrite is auditable.
  enhance: true,
};

const MAX_REFERENCES = 7;

/**
 * Records that the user chose "Skip for now".
 *
 * Without it, onboarding reappears on every launch until a key is added, which
 * turns a deliberate decision into a nag. The Generate button still says the
 * app is unconfigured, so nothing is hidden — see GenerateButton.
 */
const SKIP_KEY = "hickeyfield.onboarding.skipped";

const readSkipped = (): boolean => {
  try {
    return window.localStorage.getItem(SKIP_KEY) === "1";
  } catch {
    // Private-mode webviews can throw on access. A storage failure must not
    // stop the app booting; it just means onboarding shows again.
    return false;
  }
};

export default function App() {
  const [models, setModels] = useState<Model[]>([]);
  const [presets, setPresets] = useState<PresetFamily[]>([]);
  const [jobs, setJobs] = useState<JobSet[]>([]);

  const [useCases, setUseCases] = useState<UseCase[]>([]);
  const [tab, setTab] = useState<WorkspaceTab>("text-to-video");
  const [modelId, setModelId] = useState<string | null>(null);
  const [routeId, setRouteId] = useState<string | null>(null);
  const [presetId, setPresetId] = useState<string | null>(null);
  const [prompt, setPrompt] = useState("");
  const [settings, setSettings] = useState<GenSettings>(DEFAULT_SETTINGS);
  const [media, setMedia] = useState<MediaRef[]>([]);

  const [estimate, setEstimate] = useState<CostEstimate | null>(null);
  const [pending, setPending] = useState(false);
  /**
   * The last submit failure, shown above the feed. Held here rather than in a
   * toast because a refused generation is something the user must act on —
   * add a key, change a model, drop an attachment — not something to dismiss.
   */
  const [submitError, setSubmitError] = useState<string | null>(null);
  const [overlay, setOverlay] = useState<
    null | "presets" | "models" | "onboarding" | "settings"
  >(null);
  const [selectedId, setSelectedId] = useState<string | null>(null);

  const [catalog, setCatalog] = useState<ProviderInfo[]>(PROVIDER_CATALOG);
  const [states, setStates] = useState<KeyState[]>([]);
  // null until the first read lands. Distinguishing "not loaded" from "nothing
  // configured" is what stops the Generate button flashing "Add a provider key"
  // for one frame on every launch.
  const [configured, setConfigured] = useState<string[] | null>(null);
  const [local, setLocal] = useState<LocalEndpoints | null>(null);
  const [libraryPath, setLibraryPath] = useState<string | null>(null);

  // Object URLs for locally attached files. Held so they can be revoked;
  // leaking them keeps whole video files alive in memory for the session.
  const objectUrls = useRef<Set<string>>(new Set());

  useEffect(() => {
    void (async () => {
      const [u, m, p, j] = await Promise.all([
        listUseCases(),
        listModels(),
        listPresets(),
        listJobs(),
      ]);
      setUseCases(u);
      if (u.length > 0 && !u.some((x) => x.slug === tab)) setTab(u[0].slug);
      setModels(m);
      setPresets(p);
      setJobs(j);
      const first = m.find((x) => x.isLaunch && x.modality === "video") ?? m[0];
      if (first) {
        setModelId(first.id);
        setRouteId(first.routes[0]?.id ?? null);
      }
    })();
  }, []);

  useEffect(() => {
    const urls = objectUrls.current;
    return () => {
      for (const url of urls) URL.revokeObjectURL(url);
      urls.clear();
    };
  }, []);

  /** Re-read credential presence. Called after every change either screen makes. */
  const refreshKeys = useCallback(async () => {
    const [s, c] = await Promise.all([keyStates(), configuredProviders()]);
    setStates(s);
    setConfigured(c);
  }, []);

  const recheckLocal = useCallback(async () => {
    setLocal(await localEndpoints());
  }, []);

  useEffect(() => {
    void (async () => {
      const [info, s, c, l, root] = await Promise.all([
        providerInfo(),
        keyStates(),
        configuredProviders(),
        localEndpoints(),
        libraryRoot(),
      ]);
      setCatalog(info);
      setStates(s);
      setConfigured(c);
      setLocal(l);
      setLibraryPath(root);
      // First run: nothing can generate, so say so up front rather than letting
      // the user build a prompt that has nowhere to go.
      if (!isAppConfigured(c, l) && !readSkipped()) setOverlay("onboarding");
    })();
  }, []);

  useEffect(
    () =>
      subscribeJobs((job) => {
        setJobs((prev) => {
          const next = prev.filter((j) => j.id !== job.id);
          return [job, ...next].sort(
            (a, b) => Date.parse(b.createdAt) - Date.parse(a.createdAt),
          );
        });
      }),
    [],
  );

  const model = useMemo(
    () => models.find((m) => m.id === modelId) ?? null,
    [models, modelId],
  );
  const route = useMemo(
    () => selectRoute(model, routeId),
    [model, routeId],
  );
  // The catalogue's answer, available immediately.
  const catalogCaps = useMemo(() => capabilitiesFor(model), [model]);
  // fal's own answer, fetched when the model or the attachments change.
  // Overrides the catalogue once it lands; until then the chips still work.
  const [liveCaps, setLiveCaps] = useState<ModelCapabilities | null>(null);
  const capabilities = liveCaps ?? catalogCaps;

  useEffect(() => {
    if (!modelId) return;
    let live = true;
    setLiveCaps(null);
    void modelCapabilities(modelId, routeId, media.length > 0).then((c) => {
      // Only replace on a real answer. A null means we could not reach fal,
      // and blanking the chip row because we are offline would be worse than
      // showing the catalogue's slightly-less-certain version.
      if (live && c) setLiveCaps(c);
    });
    return () => {
      live = false;
    };
  }, [modelId, routeId, media.length]);
  const preset = useMemo(
    () => presets.find((p) => p.id === presetId) ?? null,
    [presets, presetId],
  );

  // Switching model can strand a setting the new model cannot run. Clamping
  // here rather than at submit means the chip row never displays a value that
  // would be silently rewritten on the way out.
  useEffect(() => {
    setSettings((prev) => resolveSettings(prev, capabilities));
  }, [capabilities]);

  // Cost re-estimates on every settings change, so the debounce is what keeps
  // a dragged control from firing a request per frame.
  useEffect(() => {
    if (!modelId || !route) return;
    let live = true;
    const timer = window.setTimeout(() => {
      void estimateCost({ modelId, routeId: route.id, settings })
        .then((res) => {
          if (live) setEstimate(res);
        })
        .catch(() => {
          // A failed estimate must not quote a stale price for a model the
          // user has since switched away from. Clearing it makes the button
          // say "price unavailable" rather than something wrong.
          if (live) setEstimate(null);
        });
    }, 200);
    return () => {
      live = false;
      window.clearTimeout(timer);
    };
  }, [modelId, route, settings]);

  // The roster follows the job. Filtered in Rust so the picker cannot offer
  // something the submit path would refuse — which is exactly what happened
  // when every tab showed every model and a text-only model accepted a clip.
  useEffect(() => {
    if (useCases.length === 0) return;
    let live = true;
    void modelsForUseCase(tab).then((m) => {
      if (!live || m.length === 0) return;
      setModels(m);
      // A model that cannot do the new job must not stay selected. Falling
      // back to the first launch model keeps the rail usable rather than
      // leaving it in an unrunnable state the user has to notice and fix.
      setModelId((current) => {
        if (current && m.some((x) => x.id === current)) return current;
        const next = m.find((x) => x.isLaunch) ?? m[0];
        setRouteId(next?.routes[0]?.id ?? null);
        return next?.id ?? null;
      });
    });
    return () => {
      live = false;
    };
  }, [tab, useCases.length]);

  // Whether the user has picked an aspect themselves. Once they have, nothing
  // may move it under them — an attachment that silently overrode a deliberate
  // choice would be its own inconsistency, just a politer one.
  const aspectChosen = useRef(false);

  const patchSettings = useCallback((patch: Partial<GenSettings>) => {
    if (patch.aspect !== undefined) aspectChosen.current = true;
    setSettings((prev) => ({ ...prev, ...patch }));
  }, []);

  /**
   * Follow the shape of what was attached.
   *
   * The default aspect is a landscape one, so animating a portrait photo asked
   * for 16:9 and came back landscape — the user having said nothing about
   * aspect at all, and the interface having shown them a ratio they never
   * picked. Attaching a file is the clearest statement of intent available
   * about the shape of the result, so it sets the chip, visibly and
   * overridably, rather than being quietly contradicted at submit time.
   */
  useEffect(() => {
    if (aspectChosen.current) return;
    const first = media[0];
    const url = first ? previewOf(first) : undefined;
    if (!url || capabilities.aspects.length === 0) return;

    let live = true;
    const kind = first.role.includes("video") ? "video" : "image";
    void measureAspect(url, kind).then((ratio) => {
      if (!live || ratio === null) return;
      const snapped = nearestAspect(ratio, capabilities.aspects);
      // Not through `patchSettings`: this is us following the user, not the
      // user making a choice, and recording it as a choice would freeze the
      // chip against the *next* attachment.
      if (snapped) setSettings((prev) => ({ ...prev, aspect: snapped }));
    });
    return () => {
      live = false;
    };
  }, [media, capabilities.aspects]);

  /**
   * Attach files to a role.
   *
   * `files` is only used in a plain browser. Inside the app the native dialog
   * runs instead, because it is the only thing that yields a real filesystem
   * path — and a path is what Rust needs to upload the bytes. A `blob:` URL
   * from `createObjectURL` resolves in this webview and nowhere else.
   */
  const addMedia = useCallback(
    async (role: MediaRole, files: FileList | null) => {
      const multiple = role === "reference";
      let added = await pickViaDialog(role, multiple);
      if (added === null) added = await fromFileList(role, files, multiple);
      if (added.length === 0) return;

      for (const m of added) {
        if (m.preview) objectUrls.current.add(m.preview);
      }
      setMedia((prev) => {
        const merged = mergeMedia(prev, added, multiple);
        if (!multiple) return merged;
        // Trim to the cap here rather than in the merge, so the cap stays a
        // product decision and mergeMedia stays a pure list operation.
        const refs = merged.filter((m) => m.role === "reference");
        if (refs.length <= MAX_REFERENCES) return merged;
        const keep = new Set(refs.slice(0, MAX_REFERENCES).map(sourceKey));
        return merged.filter((m) => m.role !== "reference" || keep.has(sourceKey(m)));
      });
    },
    [],
  );

  const forget = useCallback((m: MediaRef) => {
    // Leaking an object URL keeps a whole video alive in memory for the
    // session, which on a 4K clip is very visible.
    if (m.preview && objectUrls.current.delete(m.preview)) {
      URL.revokeObjectURL(m.preview);
    }
  }, []);

  const removeByRole = useCallback(
    (role: MediaRole) => {
      setMedia((prev) => {
        for (const m of prev.filter((x) => x.role === role)) forget(m);
        return prev.filter((m) => m.role !== role);
      });
    },
    [forget],
  );

  const removeByKey = useCallback(
    (key: string) => {
      setMedia((prev) => {
        for (const m of prev.filter((x) => sourceKey(x) === key)) forget(m);
        return prev.filter((m) => sourceKey(m) !== key);
      });
    },
    [forget],
  );

  const send = useCallback(
    async () => {
      if (!modelId || !route) return;
      setPending(true);
      setSubmitError(null);
      try {
        await submitJob({
          modelId,
          routeId: route.id,
          prompt,
          presetId,
          settings,
          media,
        });
        setJobs(await listJobs());
      } catch (e) {
        setSubmitError(e instanceof Error ? e.message : String(e));
        try {
          setJobs(await listJobs());
        } catch {
          // Already reporting the more useful error.
        }
      } finally {
        setPending(false);
      }
    },
    [modelId, route, prompt, presetId, settings, media],
  );

  const onSubmit = useCallback(async () => {
    if (!modelId || !route) return;
    await send();
  }, [modelId, route, send]);

  const onRerun = useCallback(
    (job: JobSet) => {
      // Reload the job's exact inputs into the rail rather than resubmitting
      // blind, so the user can change one thing before paying again.
      setModelId(job.modelId);
      setRouteId(job.route?.id ?? null);
      setPrompt(job.prompt);
      setPresetId(job.presetId ?? null);
      if (job.settings) setSettings(job.settings);
      // Restore the attachments too. Without this, rerunning an
      // image-to-video job silently resubmitted it as text-to-video — the same
      // price for a generation the user did not ask to repeat.
      setMedia(job.media ?? []);
      document.getElementById("prompt-input")?.focus();
    },
    [],
  );

  // Deletion is two steps: the card asks, the modal commits. Our accent is
  // red, so colour cannot carry destructive weight — the confirm does.
  const [pendingDelete, setPendingDelete] = useState<JobSet | null>(null);

  const onDelete = useCallback(
    (id: string) => {
      setPendingDelete(jobs.find((j) => j.id === id) ?? null);
    },
    [jobs],
  );

  const confirmDelete = useCallback(
    async (deleteFiles: boolean) => {
      const job = pendingDelete;
      setPendingDelete(null);
      if (!job) return;
      try {
        await deleteJob(job.id, deleteFiles);
        // Only drop it from the list once the shell agreed. Removing it first
        // is how a delete appeared to work and then reverted on next refresh.
        setJobs((prev) => prev.filter((j) => j.id !== job.id));
        if (selectedId === job.id) setSelectedId(null);
      } catch (e) {
        setSubmitError(e instanceof Error ? e.message : String(e));
      }
    },
    [pendingDelete, selectedId],
  );

  // Selection is the pairing mechanism between the two independently
  // scrolling columns. `nearest` means an already-visible counterpart does not
  // jump, which matters because hovering a meta card selects it.
  useEffect(() => {
    if (!selectedId) return;
    for (const attr of ["data-job-anchor", "data-meta-anchor"]) {
      document
        .querySelector(`[${attr}="${selectedId}"]`)
        ?.scrollIntoView({ block: "nearest", behavior: "smooth" });
    }
  }, [selectedId]);

  const runningCount = jobs.filter((j) => isRunning(j.status)).length;
  const needsSetup = configured !== null && !isAppConfigured(configured, local);

  const skipOnboarding = useCallback(() => {
    try {
      window.localStorage.setItem(SKIP_KEY, "1");
    } catch {
      // Not being able to remember the skip is a nuisance, not a failure.
    }
    setOverlay(null);
  }, []);

  return (
    <div className="shell">
      <header className="titlebar" data-tauri-drag-region>
        <span className="wordmark">HICKEYFIELD</span>
        <span className="titlebar-meta">
          {runningCount > 0 ? (
            <span className="titlebar-running">{runningCount} running</span>
          ) : null}
          <span className="titlebar-mode">
            {isDesktop() ? "Desktop" : "Browse mode · sample data"}
          </span>
          <button
            type="button"
            className="btn btn-icon btn-ghost titlebar-action"
            aria-label="Settings"
            onClick={() => setOverlay("settings")}
          >
      <ConfirmDelete
        open={pendingDelete !== null}
        hasFiles={Boolean(
          pendingDelete?.results?.some((r) => Boolean(r.localPath)),
        )}
        onCancel={() => setPendingDelete(null)}
        onConfirm={(files) => void confirmDelete(files)}
      />

      <SettingsIcon size={18} />
          </button>
        </span>
      </header>

      <div className="workspace">
        <SettingsRail
          useCases={useCases}
          tab={tab}
          onTabChange={setTab}
          preset={preset}
          model={model}
          route={route}
          capabilities={capabilities}
          settings={settings}
          onSettingsChange={patchSettings}
          prompt={prompt}
          onPromptChange={setPrompt}
          media={media}
          onMediaAdd={addMedia}
          onMediaRemoveRole={removeByRole}
          onMediaRemoveKey={removeByKey}
          estimate={estimate}
          pending={pending}
          needsSetup={needsSetup}
          onOpenPresets={() => setOverlay("presets")}
          onOpenModels={() => setOverlay("models")}
          onOpenSetup={() => setOverlay("onboarding")}
          onSubmit={() => void onSubmit()}
        />

        <ResultsFeed
          error={submitError}
          onDismissError={() => setSubmitError(null)}
          jobs={jobs}
          selectedId={selectedId}
          onSelect={setSelectedId}
          onCancel={(id) => void cancelJob(id)}
          onFocusPrompt={() => document.getElementById("prompt-input")?.focus()}
        />

        <MetaRail
          jobs={jobs}
          models={models}
          selectedId={selectedId}
          onSelect={setSelectedId}
          onRerun={onRerun}
          onDelete={onDelete}
        />
      </div>

      <PresetPicker
        open={overlay === "presets"}
        onClose={() => setOverlay(null)}
        presets={presets}
        models={models}
        activeModelId={modelId}
        onPick={(p) => setPresetId(p.id)}
      />

      <ModelPicker
        open={overlay === "models"}
        onClose={() => setOverlay(null)}
        models={models}
        selectedModelId={modelId}
        selectedRouteId={route?.id ?? null}
        onSelect={(m, r) => {
          setModelId(m);
          setRouteId(r);
        }}
      />

      <Onboarding
        open={overlay === "onboarding"}
        onClose={() => setOverlay(null)}
        onSkip={skipOnboarding}
        catalog={catalog}
        states={states}
        local={local}
        onChanged={() => void refreshKeys()}
      />

      <Settings
        open={overlay === "settings"}
        onClose={() => setOverlay(null)}
        catalog={catalog}
        states={states}
        local={local}
        libraryPath={libraryPath}
        onChanged={() => void refreshKeys()}
        onRecheckLocal={recheckLocal}
        onRunOnboarding={() => setOverlay("onboarding")}
      />
    </div>
  );
}
