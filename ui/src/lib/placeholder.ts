/**
 * Deterministic inline artwork for previews.
 *
 * Every preview asset in this repo has to be ours, and no build-time network
 * fetch is allowed, so browse-mode previews are generated as SVG data URIs
 * from the item's id. Deterministic so a preset keeps the same look between
 * renders, and inline so nothing is ever requested from a third-party CDN.
 */

function hash(seed: string): number {
  let h = 2166136261;
  for (let i = 0; i < seed.length; i += 1) {
    h ^= seed.charCodeAt(i);
    h = Math.imul(h, 16777619);
  }
  return Math.abs(h);
}

export function seededHue(seed: string): number {
  return hash(seed) % 360;
}

export function gradientDataUri(seed: string, w = 640, h = 360): string {
  const n = hash(seed);
  const hue = n % 360;
  const hue2 = (hue + 40 + (n % 90)) % 360;
  const angle = n % 180;
  const cx = 20 + (n % 60);
  const cy = 20 + ((n >> 3) % 60);
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${w} ${h}" width="${w}" height="${h}">
<defs>
<linearGradient id="g" gradientTransform="rotate(${angle} .5 .5)">
<stop offset="0" stop-color="hsl(${hue} 62% 24%)"/>
<stop offset="1" stop-color="hsl(${hue2} 48% 10%)"/>
</linearGradient>
<radialGradient id="r" cx="${cx}%" cy="${cy}%" r="60%">
<stop offset="0" stop-color="hsl(${hue2} 90% 62%)" stop-opacity=".55"/>
<stop offset="1" stop-color="hsl(${hue} 80% 40%)" stop-opacity="0"/>
</radialGradient>
</defs>
<rect width="${w}" height="${h}" fill="url(#g)"/>
<rect width="${w}" height="${h}" fill="url(#r)"/>
</svg>`;
  return `data:image/svg+xml,${encodeURIComponent(svg)}`;
}

/**
 * The animated grain layer of the rainbow placeholder: fractal noise whose
 * seed animates 1 to 65 over 8s. Kept as a data URI because an <animate> on
 * feTurbulence is the only way to get a moving grain field without a canvas
 * loop burning a core while a generation runs.
 */
export const GRAIN_DATA_URI = `data:image/svg+xml,${encodeURIComponent(
  `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 400 400"><defs><filter id="n"><feTurbulence type="fractalNoise" baseFrequency="0.65" numOctaves="3" stitchTiles="stitch"><animate attributeName="seed" from="1" to="65" dur="8s" repeatCount="indefinite"/></feTurbulence><feColorMatrix type="saturate" values="0"/></filter></defs><rect width="400" height="400" filter="url(#n)" fill="#000"/></svg>`,
)}`;
