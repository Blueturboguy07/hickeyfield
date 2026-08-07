/**
 * Browse-mode data.
 *
 * The Rust core is a separate process that only exists inside the Tauri shell.
 * Every bridge call falls back here so `pnpm dev` in a plain browser gives a
 * fully populated, clickable app — including a generation that actually
 * progresses — instead of a wall of empty states. Nothing in here is imported
 * by the desktop path once the commands answer.
 */

import type {
  ModelCapabilities,
  CostEstimate,
  JobSet,
  Model,
  PresetFamily,
  SubmitInput,
} from "./types";
import { gradientDataUri } from "./lib/placeholder";

/**
 * Mock capabilities. Spelled through a helper so the mock roster cannot drift
 * from the real `ModelCapabilities` shape — which is exactly how the UI ended
 * up assuming defaults for models that declare none.
 */
const caps = (over: Partial<ModelCapabilities> = {}): ModelCapabilities => ({
  supportsDuration: (over.durations?.length ?? 0) > 0,
  durations: [],
  supportsResolution: (over.resolutions?.length ?? 0) > 0,
  resolutions: [],
  supportsAspect: (over.aspects?.length ?? 0) > 0,
  aspects: ["16:9", "9:16", "1:1"],
  audio: false,
  constraints: [],
  ...over,
});

const route = (provider: string, slug: string) => ({
  id: `${provider}:${slug}`,
  provider,
  slug,
});

export const MOCK_MODELS: Model[] = [
  {
    id: "kling_3_0",
    displayName: "Kling 3.0",
    modality: "video",
    isLaunch: true,
    subtitle: "Cinematic motion with strong subject coherence",
    routes: [route("fal", "kling-video/v3/standard"), route("vaig", "kling-3")],
    capabilities: caps({
      durations: [5, 10],
      resolutions: ["720p", "1080p"],
      aspects: ["16:9", "9:16", "1:1"],
      audio: true,
    }),
  },
  {
    id: "seedance_2_0",
    displayName: "Seedance 2.0",
    modality: "video",
    isLaunch: true,
    subtitle: "Fast multi-shot generation, token priced by resolution",
    routes: [
      route("fal", "bytedance/seedance/v2/pro"),
      route("vaig", "seedance-2"),
    ],
    capabilities: caps({
      durations: [4, 6, 8],
      resolutions: ["480p", "720p", "1080p"],
      aspects: ["16:9", "9:16", "4:3", "1:1"],
      audio: true,
    }),
  },
  {
    id: "wan_2_7",
    displayName: "Wan 2.7",
    modality: "video",
    isLaunch: true,
    subtitle: "Open weights, runs locally or hosted",
    routes: [route("fal", "wan/v2.7/t2v"), route("local", "comfyui/wan-2.7")],
    capabilities: caps({
      durations: [5, 8],
      resolutions: ["480p", "720p"],
      aspects: ["16:9", "9:16", "1:1"],
      audio: false,
    }),
  },
  {
    id: "veo_3_1_fast",
    displayName: "Veo 3.1 Fast",
    modality: "video",
    isLaunch: true,
    subtitle: "Native audio, billed on wall-clock output seconds",
    routes: [route("google", "veo-3.1-fast"), route("vaig", "veo-3-1-fast")],
    capabilities: caps({
      durations: [4, 6, 8],
      resolutions: ["720p", "1080p"],
      aspects: ["16:9", "9:16"],
      audio: true,
    }),
  },
  {
    id: "gemini_omni",
    displayName: "Gemini Omni",
    modality: "video",
    isLaunch: false,
    subtitle: "Long-context video from mixed references",
    routes: [route("google", "gemini-omni-video")],
    capabilities: caps({
      durations: [5, 8, 10],
      resolutions: ["720p", "1080p"],
      aspects: ["16:9", "9:16", "21:9"],
      audio: true,
    }),
  },
  {
    id: "nano_banana_pro",
    displayName: "Nano Banana Pro",
    modality: "image",
    isLaunch: true,
    subtitle: "Editing and composition with up to 14 references",
    routes: [route("google", "nano-banana-pro"), route("fal", "nano-banana")],
    capabilities: caps({ resolutions: ["1K", "2K", "4K"] }),
  },
  {
    id: "seedream_5_pro",
    displayName: "Seedream 5.0 Pro",
    modality: "image",
    isLaunch: true,
    subtitle: "High-detail stills, four images for the price of one",
    routes: [route("fal", "bytedance/seedream/v5/pro")],
    capabilities: caps({ resolutions: ["1K", "2K", "4K"] }),
  },
  {
    id: "flux_2_pro",
    displayName: "FLUX.2 Pro",
    modality: "image",
    isLaunch: true,
    subtitle: "The photoreal default when Styles are involved",
    routes: [route("bfl", "flux-2-pro"), route("fal", "flux/v2/pro")],
    capabilities: caps({ resolutions: ["1K", "2K"] }),
  },
  {
    id: "recraft_v4_1",
    displayName: "Recraft v4.1",
    modality: "image",
    isLaunch: false,
    subtitle: "Vector-aware brand and layout work",
    routes: [route("recraft", "v4.1")],
    capabilities: caps({ resolutions: ["1K", "2K"] }),
  },
  {
    id: "elevenlabs_v3",
    displayName: "ElevenLabs v3",
    modality: "audio",
    isLaunch: false,
    subtitle: "Speech and voice cloning",
    routes: [route("fal", "elevenlabs/tts/v3")],
    capabilities: caps({ durations: [], resolutions: [], aspects: [], audio: true }),
  },
];

interface PresetSeed {
  id: string;
  name: string;
  category: string;
  tags: string[];
  description: string;
  models?: string[];
}

const PRESET_SEEDS: PresetSeed[] = [
  {
    id: "general",
    name: "General",
    category: "Trending",
    tags: ["manual", "default"],
    description: "No preset styling. Your prompt, unmodified, with full manual control.",
  },
  {
    id: "orbit-360",
    name: "Orbit 360",
    category: "Camera Control",
    tags: ["orbit", "camera", "rotation"],
    description: "A full circular orbit around the subject at a fixed radius.",
    models: ["kling_3_0", "seedance_2_0", "wan_2_7"],
  },
  {
    id: "earth-zoom",
    name: "Earth Zoom",
    category: "Camera Control",
    tags: ["zoom", "camera", "scale"],
    description: "A continuous pull from ground level out to orbital altitude.",
    models: ["kling_3_0", "veo_3_1_fast"],
  },
  {
    id: "crash-dolly",
    name: "Crash Dolly",
    category: "Camera Control",
    tags: ["dolly", "camera", "push"],
    description: "A hard accelerating push straight into the subject.",
    models: ["kling_3_0", "seedance_2_0"],
  },
  {
    id: "handheld-follow",
    name: "Handheld Follow",
    category: "Camera Control",
    tags: ["handheld", "camera", "tracking"],
    description: "Loose operator-held tracking shot with natural sway.",
    models: ["wan_2_7", "seedance_2_0"],
  },
  {
    id: "crane-reveal",
    name: "Crane Reveal",
    category: "Camera Control",
    tags: ["crane", "camera", "reveal"],
    description: "Rising crane move that opens onto the wider scene.",
  },
  {
    id: "datamosh",
    name: "Datamosh",
    category: "Effects",
    tags: ["glitch", "vfx", "transition"],
    description: "Pixels bleed and smear between frames into distorted transitions.",
    models: ["seedance_2_0", "wan_2_7"],
  },
  {
    id: "ice-statue",
    name: "Ice Statue",
    category: "Effects",
    tags: ["freeze", "vfx", "transform"],
    description: "The subject crystallises into carved ice from the feet up.",
    models: ["kling_3_0"],
  },
  {
    id: "cardboard-cutout",
    name: "Cardboard Cutout",
    category: "Effects",
    tags: ["paper", "vfx", "transform"],
    description: "The scene flattens into layered cardboard shapes.",
  },
  {
    id: "sticker-peel",
    name: "Sticker Peel",
    category: "Effects",
    tags: ["peel", "vfx", "transition"],
    description: "The frame lifts and peels away like a vinyl sticker.",
    models: ["seedance_2_0"],
  },
  {
    id: "clay-figurine",
    name: "Clay Figurine",
    category: "Effects",
    tags: ["clay", "stopmotion", "transform"],
    description: "Everything becomes hand-modelled clay under stop-motion light.",
  },
  {
    id: "free-fall",
    name: "Free Fall",
    category: "Viral",
    tags: ["action", "drop", "gravity"],
    description: "The subject plummets while the camera falls alongside.",
    models: ["kling_3_0", "veo_3_1_fast"],
  },
  {
    id: "red-carpet",
    name: "Red Carpet",
    category: "Viral",
    tags: ["paparazzi", "flash", "celebrity"],
    description: "Step-and-repeat arrival under a wall of camera flashes.",
    models: ["kling_3_0"],
  },
  {
    id: "action-figure",
    name: "Action Figure",
    category: "Viral",
    // `chain` families run a fixed multi-step prompt of their own and reject a
    // user prompt outright, which is why the rail disables the prompt box.
    tags: ["toy", "packaging", "product", "chain"],
    description:
      "The subject arrives boxed as a collectible figure. Takes exactly one image, no prompt.",
  },
  {
    id: "night-vision",
    name: "Night Vision",
    category: "Viral",
    tags: ["ir", "surveillance", "green"],
    description: "Infrared surveillance look with bloom and sensor grain.",
    models: ["wan_2_7", "seedance_2_0"],
  },
  {
    id: "office-cctv",
    name: "Office CCTV",
    category: "Viral",
    tags: ["cctv", "surveillance", "fixed"],
    description: "Fixed ceiling-corner camera with timestamp burn-in.",
  },
  {
    id: "neon-city",
    name: "Neon City",
    category: "Mood",
    tags: ["neon", "night", "urban"],
    description: "Wet asphalt, saturated signage and long reflections.",
    models: ["kling_3_0", "seedance_2_0", "veo_3_1_fast"],
  },
  {
    id: "summer-haze",
    name: "Summer Haze",
    category: "Mood",
    tags: ["warm", "grain", "nostalgic"],
    description: "Blown-out highlights and warm haze on expired film stock.",
  },
  {
    id: "in-the-dark",
    name: "In The Dark",
    category: "Mood",
    tags: ["low-key", "contrast", "noir"],
    description: "Single hard source against deep shadow, most of the frame unlit.",
    models: ["kling_3_0"],
  },
  {
    id: "foggy-morning",
    name: "Foggy Morning",
    category: "Mood",
    tags: ["fog", "soft", "cold"],
    description: "Flat cold light diffused through heavy ground fog.",
  },
  {
    id: "cel-shaded",
    name: "Cel Shaded",
    category: "Anime",
    tags: ["anime", "flat", "linework"],
    description: "Hard cel shading with visible ink lines and limited palette.",
    models: ["wan_2_7", "seedance_2_0"],
  },
  {
    id: "sakura-drift",
    name: "Sakura Drift",
    category: "Anime",
    tags: ["anime", "petals", "romance"],
    description: "Petals crossing frame in slow parallax over a still subject.",
  },
  {
    id: "mecha-launch",
    name: "Mecha Launch",
    category: "Anime",
    tags: ["anime", "mecha", "action"],
    description: "Low-angle takeoff with speed lines and impact dust.",
    models: ["kling_3_0", "wan_2_7"],
  },
  {
    id: "kinetic-titles",
    name: "Kinetic Titles",
    category: "Trending",
    tags: ["type", "motion", "graphic"],
    description: "Typography enters on the beat and settles into the composition.",
  },
  {
    id: "product-spin",
    name: "Product Spin",
    category: "Trending",
    tags: ["product", "commercial", "turntable"],
    description: "Seamless turntable loop on a lit sweep background.",
    models: ["seedance_2_0", "veo_3_1_fast"],
  },
  {
    id: "soft-portrait",
    name: "Soft Portrait",
    category: "Trending",
    tags: ["beauty", "portrait", "skin"],
    description: "Beauty-dish key with gentle falloff and preserved skin texture.",
  },
];

export const MOCK_PRESETS: PresetFamily[] = PRESET_SEEDS.map((seed) => ({
  id: seed.id,
  displayName: seed.name,
  category: seed.category,
  tags: seed.tags,
  description: seed.description,
  variants: seed.models?.map((modelId) => ({
    modelId,
    previewUrl: gradientDataUri(`${seed.id}:${modelId}`, 360, 640),
  })),
}));

export const presetPreview = (presetId: string, w = 360, h = 640): string =>
  gradientDataUri(`preview:${presetId}`, w, h);

/**
 * Per-second USD by model, as a stand-in for the live price feeds.
 *
 * `null` is deliberate on Wan: several fal `pricingInfoOverride` fields ship
 * blank, and the unknown-price path has to be reachable in browse mode or it
 * only ever gets exercised in production.
 */
const RATE_PER_SECOND: Record<string, number | null> = {
  kling_3_0: 0.28,
  seedance_2_0: 0.19,
  wan_2_7: null,
  veo_3_1_fast: 0.15,
  gemini_omni: 0.32,
};

const IMAGE_PRICE: Record<string, number> = {
  nano_banana_pro: 0.039,
  seedream_5_pro: 0.012,
  flux_2_pro: 0.04,
  recraft_v4_1: 0.008,
};

const RESOLUTION_MULTIPLIER: Record<string, number> = {
  "480p": 0.6,
  "720p": 1,
  "1080p": 2.25,
  "1K": 1,
  "2K": 1.8,
  "4K": 3.2,
};

/** fal bills a floor on some hosted video models regardless of clip length. */
const MINIMUM_USD = 0.35;

export function mockEstimate(
  modelId: string,
  settings: { duration: number; resolution: string; audio: boolean },
): CostEstimate | null {
  const image = IMAGE_PRICE[modelId];
  if (image !== undefined) {
    const mult = RESOLUTION_MULTIPLIER[settings.resolution] ?? 1;
    return {
      usd: image * mult,
      basis: `1 image at ${settings.resolution}`,
    };
  }

  const rate = RATE_PER_SECOND[modelId];
  if (rate === null || rate === undefined) return null;

  const mult = RESOLUTION_MULTIPLIER[settings.resolution] ?? 1;
  const audioMult = settings.audio ? 1.5 : 1;
  const raw = rate * settings.duration * mult * audioMult;
  const usd = Math.max(raw, MINIMUM_USD);
  return {
    usd,
    basis: `${settings.duration}s at ${settings.resolution}${
      settings.audio ? " with audio" : ""
    }`,
    minimumApplied: usd > raw,
  };
}

const iso = (minutesAgo: number) =>
  new Date(Date.now() - minutesAgo * 60_000).toISOString();

export const MOCK_JOBS: JobSet[] = [
  {
    id: "job-3",
    modelId: "seedance_2_0",
    status: "in_progress",
    prompt:
      "A lone cyclist crossing a rain-slicked intersection at night, headlights raking across the frame, steam rising from a grate.",
    createdAt: iso(1),
    results: [],
    route: route("fal", "bytedance/seedance/v2/pro"),
    estimatedUsd: 1.71,
    presetId: "neon-city",
    presetName: "Neon City",
    settings: {
      duration: 8,
      resolution: "1080p",
      aspect: "16:9",
      audio: true,
      enhance: true,
      seed: 377382,
      steps: 20,
    },
    media: [
      {
        role: "start",
        source: { kind: "url", url: gradientDataUri("job-3-start", 240, 240) },
        name: "intersection.png",
      },
    ],
    enhancerVersion: "hickeyfield-enhance-1.2",
  },
  {
    id: "job-2",
    modelId: "kling_3_0",
    status: "completed",
    prompt:
      "Slow orbit around a weathered bronze statue in an empty museum hall, dust suspended in a shaft of window light.",
    enhancedPrompt:
      "Slow 180-degree orbit around a weathered bronze statue in an empty museum hall. Volumetric shaft of window light, suspended dust motes, marble floor reflections, shallow depth of field, 35mm.",
    createdAt: iso(14),
    results: [
      {
        url: gradientDataUri("job-2-result", 1280, 720),
        kind: "image",
        width: 1280,
        height: 720,
      },
    ],
    route: route("fal", "kling-video/v3/standard"),
    estimatedUsd: 1.4,
    actualUsd: 1.4,
    presetId: "orbit-360",
    presetName: "Orbit 360",
    settings: {
      duration: 5,
      resolution: "1080p",
      aspect: "16:9",
      audio: false,
      enhance: true,
      seed: 918204,
      steps: 20,
    },
    media: [],
    enhancerVersion: "hickeyfield-enhance-1.2",
  },
  {
    id: "job-1",
    modelId: "wan_2_7",
    status: "completed",
    prompt:
      "Portrait of a street vendor lit by a single sodium lamp, shot from below, steam from the cart drifting through frame.",
    createdAt: iso(41),
    results: [
      {
        url: gradientDataUri("job-1-result", 720, 1280),
        kind: "image",
        width: 720,
        height: 1280,
      },
    ],
    route: route("local", "comfyui/wan-2.7"),
    // Unknown estimate on a route with no published price. It still ran.
    estimatedUsd: null,
    actualUsd: 0,
    presetId: null,
    settings: {
      duration: 5,
      resolution: "720p",
      aspect: "9:16",
      audio: false,
      enhance: false,
      seed: 44117,
      steps: 30,
    },
    media: [
      {
        role: "reference",
        source: { kind: "url", url: gradientDataUri("job-1-ref", 240, 240) },
        name: "vendor-ref.jpg",
      },
    ],
    enhancerVersion: null,
  },
];

// ── Mock job engine ────────────────────────────────────────────────────────
// A submit in browse mode has to visibly do something, otherwise the running
// card, the blurred backdrop and the indeterminate loader are unreachable
// without the Rust core attached.

type Listener = (job: JobSet) => void;

const listeners = new Set<Listener>();
let store: JobSet[] = [...MOCK_JOBS];
let counter = 0;

export function mockListJobs(): JobSet[] {
  return store;
}

export function mockSubscribe(cb: Listener): () => void {
  listeners.add(cb);
  return () => {
    listeners.delete(cb);
  };
}

function emit(job: JobSet) {
  store = [job, ...store.filter((j) => j.id !== job.id)].sort(
    (a, b) => Date.parse(b.createdAt) - Date.parse(a.createdAt),
  );
  for (const cb of listeners) cb(job);
}

export function mockSubmit(input: SubmitInput): string {
  counter += 1;
  const id = `job-mock-${counter}`;
  const model = MOCK_MODELS.find((m) => m.id === input.modelId);
  const routeSpec = model?.routes.find((r) => r.id === input.routeId) ??
    model?.routes[0] ?? { id: input.routeId, provider: "unknown", slug: "" };
  const estimate = mockEstimate(input.modelId, input.settings);
  const preset = MOCK_PRESETS.find((p) => p.id === input.presetId);

  const base: JobSet = {
    id,
    modelId: input.modelId,
    status: "queued",
    prompt: input.prompt,
    createdAt: new Date().toISOString(),
    results: [],
    route: routeSpec,
    estimatedUsd: estimate?.usd ?? null,
    presetId: input.presetId,
    presetName: preset?.displayName ?? null,
    settings: { ...input.settings, seed: Math.floor(Math.random() * 999999) },
    media: input.media,
    enhancerVersion: input.settings.enhance ? "hickeyfield-enhance-1.2" : null,
  };
  emit(base);

  window.setTimeout(() => {
    emit({
      ...base,
      status: "in_progress",
      enhancedPrompt: input.settings.enhance
        ? `${input.prompt} Shot on 35mm, shallow depth of field, motivated practical lighting.`
        : undefined,
    });
  }, 1200);

  window.setTimeout(() => {
    const portrait = input.settings.aspect === "9:16";
    emit({
      ...base,
      status: "completed",
      enhancedPrompt: input.settings.enhance
        ? `${input.prompt} Shot on 35mm, shallow depth of field, motivated practical lighting.`
        : undefined,
      actualUsd: estimate?.usd ?? null,
      results: [
        {
          url: gradientDataUri(id, portrait ? 720 : 1280, portrait ? 1280 : 720),
          kind: "image",
          width: portrait ? 720 : 1280,
          height: portrait ? 1280 : 720,
        },
      ],
    });
  }, 6500);

  return id;
}

export function mockCancel(jobSetId: string): void {
  const job = store.find((j) => j.id === jobSetId);
  if (!job) return;
  emit({ ...job, status: "canceled" });
}
