// Record the README's animations from the running app.
//
//     cd apps/desktop
//     npm install --no-save puppeteer-core gifenc pngjs
//     npm run dev                       # in another terminal
//     node ../../scripts/record-readme.mjs            # all three
//     node ../../scripts/record-readme.mjs codes      # or just the ones named
//
// What it makes, in docs/screenshots:
//
//   codes.gif            the Codes screen, with the vehicle turning in it
//   hologram-shapes.gif  a car, an SUV, a pickup and a van, each with a part lit
//   live.gif             the Live data screen, streaming
//
// `codes` and `live` are the real app, so they need a core running and
// connected to the virtual truck with the exhaust filter fault:
//
//     cargo run -p aim-api -- --simulator --vehicle f250 --scenario dpf-regen
//     (then press Connect once in a browser at http://localhost:5173)
//
// Why it works this way. The vehicle is drawn only when it is told to, at the
// moment it is told (see `holoRecording` in VehicleHologram.tsx). So frame 37
// is the same picture every run, and a clip that lasts exactly as long as the
// animation takes to repeat has no seam. A screen recording has neither
// property, and needs somebody to crop it. Live data is the exception: it is
// whatever the virtual truck sent while this ran, captured as it arrived.
//
// The three packages are not in package.json on purpose: they are for making
// pictures of the app, not for building it. A browser is not downloaded
// either. This drives the Edge or Chrome already installed; set BROWSER to its
// path if it is not found.

import { spawn } from "node:child_process";
import { createRequire } from "node:module";
import { existsSync, mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const require = createRequire(path.join(root, "apps/desktop/package.json"));
const puppeteer = require("puppeteer-core");
const { PNG } = require("pngjs");
const { GIFEncoder, quantize, applyPalette } = require("gifenc");

const BASE = process.env.README_URL ?? "http://localhost:5173";
const OUT = path.join(root, "docs/screenshots");
const FPS = 10;
/** The length everything in the scene repeats over: `LOOP_SECONDS` in scene.ts. */
const LOOP_SECONDS = 6;
const FRAMES = FPS * LOOP_SECONDS;
/** The app's window in the recordings. */
const APP = { width: 1180, height: 900 };

const BROWSERS = [
  process.env.BROWSER,
  "C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe",
  "C:/Program Files/Microsoft/Edge/Application/msedge.exe",
  "C:/Program Files/Google/Chrome/Application/chrome.exe",
  "/usr/bin/google-chrome",
  "/usr/bin/chromium",
  "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
].filter(Boolean);

const wait = (ms) => new Promise((done) => setTimeout(done, ms));

async function shot(page) {
  return PNG.sync.read(Buffer.from(await page.screenshot({ type: "png" }))).data;
}

/** Draw every hologram on the page as it is at frame `i` of the loop. */
async function step(page, i) {
  await page.evaluate(
    (seconds, turn) =>
      new Promise((done) => {
        for (const hologram of window.holoRecording ?? []) hologram.step(seconds, turn);
        // Two frames: one for the canvas to be presented, one to be sure.
        requestAnimationFrame(() => requestAnimationFrame(done));
      }),
    i / FPS,
    (i / FRAMES) * Math.PI * 2,
  );
}

/**
 * Frames as a GIF, with one palette for the whole clip so colours do not
 * shimmer, and each frame holding only the pixels that changed since the one
 * before. A screen of text with one moving picture in it is then about the
 * size of the picture, not of the screen.
 *
 * `frames` is `[{ rgba, delay }]`, delay in milliseconds.
 */
function encode(frames, width, height) {
  const every = Math.max(1, Math.floor(frames.length / 8));
  const sample = Buffer.concat(frames.filter((_, i) => i % every === 0).map((f) => f.rgba));
  const palette = quantize(sample, 255, { format: "rgb565" });
  const clear = palette.length;
  palette.push([0, 0, 0]);

  const gif = GIFEncoder();
  let shown = null;
  frames.forEach(({ rgba, delay }, i) => {
    const index = applyPalette(rgba, palette, "rgb565");
    if (shown === null) {
      shown = Uint8Array.from(index);
      gif.writeFrame(index, width, height, { palette, delay, repeat: 0 });
      return;
    }
    for (let p = 0; p < index.length; p++) {
      if (index[p] === shown[p]) index[p] = clear;
      else shown[p] = index[p];
    }
    gif.writeFrame(index, width, height, { delay, transparent: true, transparentIndex: clear, dispose: 1 });
  });
  gif.finish();
  return Buffer.from(gif.bytes());
}

function save(file, frames, width, height) {
  const bytes = encode(frames, width, height);
  writeFileSync(path.join(OUT, file), bytes);
  console.log(`\n  wrote docs/screenshots/${file}: ${frames.length} frames, ${(bytes.length / 1e6).toFixed(2)} MB`);
}

/**
 * The app, connected, with the engine computer selected and `tab` open.
 * `recording` makes its holograms draw only when stepped.
 */
async function openApp(browser, tab, recording) {
  const page = await browser.newPage();
  await page.setViewport({ ...APP, deviceScaleFactor: 1 });
  await page.evaluateOnNewDocument((manual) => {
    localStorage.setItem("aim.body-shape", "truck");
    if (manual) window.holoRecording = [];
  }, recording);
  await page.goto(BASE, { waitUntil: "networkidle0" });
  // The splash leaves by itself once the core has answered and the vehicle's
  // computers are listed. Nothing underneath it can be pressed before then.
  await page.waitForFunction(
    () => !document.querySelector(".splash-scrim") && !!document.querySelector("[role=tab]"),
    { timeout: 60_000 },
  );
  await page.evaluate((wanted) => {
    const say = (text) => {
      throw new Error(`${text}. Is the core running and connected to the virtual truck?`);
    };
    const module = [...document.querySelectorAll("div, li, button")].find(
      (el) => el.childElementCount <= 2 && el.textContent.trim().startsWith("SIM ENGINE CONTROL"),
    );
    if (!module) say("No engine computer is listed");
    module.click();
    const tab = [...document.querySelectorAll("[role=tab]")].find((el) => el.textContent.trim() === wanted);
    if (!tab) say(`There is no ${wanted} tab`);
    tab.click();
  }, tab);
  // Nothing on the page should move except what is being recorded.
  await page.addStyleTag({ content: "*, *::before, *::after { animation: none !important; transition: none !important; }" });
  return page;
}

const CLIPS = {
  /** The Codes screen: the codes, and the vehicle turning underneath them. */
  async codes(browser) {
    const page = await openApp(browser, "Codes", true);
    await page.waitForSelector(".holo-canvas", { timeout: 60_000 });
    await page.evaluate(() => {
      const where = [...document.querySelectorAll(".where")].find((el) => el.offsetParent !== null);
      where?.scrollIntoView({ block: "end" });
    });
    await wait(800);
    const frames = [];
    for (let i = 0; i < FRAMES; i++) {
      await step(page, i);
      frames.push({ rgba: await shot(page), delay: 1000 / FPS });
      process.stdout.write(`\r  codes: frame ${i + 1}/${FRAMES}`);
    }
    await page.close();
    save("codes.gif", frames, APP.width, APP.height);
  },

  /** Four vehicles, a quarter of a turn each, each with a different part lit. */
  async shapes(browser) {
    const size = { width: 720, height: 405 };
    const parts = [
      { shape: "car", region: "exhaust", part: "catalytic_converter", what: "Catalytic converter" },
      { shape: "suv", region: "engine_bay", part: "ignition_coil", what: "Ignition coil" },
      { shape: "truck", region: "fuel_system", part: "fuel_cap", what: "Fuel filler cap" },
      { shape: "van", region: "electrical", part: "battery", what: "Battery and charging" },
    ];
    const page = await browser.newPage();
    await page.setViewport({ ...size, deviceScaleFactor: 1 });
    const frames = [];
    const share = FRAMES / parts.length;
    for (const [n, spec] of parts.entries()) {
      await page.goto(`${BASE}/hologram-preview.html?${new URLSearchParams({ record: "", ...spec })}`, {
        waitUntil: "networkidle0",
      });
      await page.waitForFunction(() => (window.holoRecording?.length ?? 0) > 0, { timeout: 30_000 });
      for (let i = n * share; i < (n + 1) * share; i++) {
        await step(page, i);
        frames.push({ rgba: await shot(page), delay: 1000 / FPS });
        process.stdout.write(`\r  shapes: frame ${i + 1}/${FRAMES}`);
      }
    }
    await page.close();
    save("hologram-shapes.gif", frames, size.width, size.height);
  },

  /** Live data, as it streams. Not a loop: it is what the truck sent. */
  async live(browser) {
    const page = await openApp(browser, "Live data", false);
    await page.waitForFunction(
      () => [...document.querySelectorAll("button")].some((b) => b.textContent.trim() === "Start" && !b.disabled),
      { timeout: 60_000 },
    );
    await page.evaluate(() => {
      [...document.querySelectorAll("button")].find((b) => b.textContent.trim() === "Start").click();
    });
    // The graphs hold thirty seconds. Wait for them to fill, so the recording
    // is of full graphs moving and not of graphs growing from one edge.
    await wait(33_000);
    const frames = [];
    const seconds = 8;
    let last = Date.now();
    while (frames.length < seconds * 5) {
      const rgba = await shot(page);
      const now = Date.now();
      // The delay a frame is shown for is how long it really was until the
      // next one, so the recording runs at the speed the screen did.
      frames.push({ rgba, delay: Math.max(60, now - last) });
      last = now;
      process.stdout.write(`\r  live: frame ${frames.length}/${seconds * 5}`);
      await wait(Math.max(0, 200 - (Date.now() - now)));
    }
    await page.close();
    save("live.gif", frames, APP.width, APP.height);
  },
};

const wanted = process.argv.slice(2);
const unknown = wanted.filter((name) => !(name in CLIPS));
if (unknown.length) {
  console.error(`No clip called ${unknown.join(", ")}. There are: ${Object.keys(CLIPS).join(", ")}.`);
  process.exit(1);
}
const executablePath = BROWSERS.find((candidate) => existsSync(candidate));
if (!executablePath) {
  console.error("No Edge or Chrome found. Set BROWSER to the path of one.");
  process.exit(1);
}
mkdirSync(OUT, { recursive: true });

// Started by hand and connected to, rather than launched by the library. On
// Windows, Edge's own launcher starts the browser and exits at once, which a
// library waiting on that process reads as a browser that failed to start.
const PORT = 9333;
spawn(
  executablePath,
  [
    "--headless=new",
    `--remote-debugging-port=${PORT}`,
    `--user-data-dir=${mkdtempSync(path.join(os.tmpdir(), "readme-recording-"))}`,
    "--no-first-run",
    "--hide-scrollbars",
    // A machine with no usable graphics card still has to be able to draw this.
    "--enable-unsafe-swiftshader",
    "--ignore-gpu-blocklist",
    "about:blank",
  ],
  { stdio: "ignore", detached: true },
).unref();
let browser = null;
for (let attempt = 0; attempt < 40 && !browser; attempt++) {
  await wait(250);
  browser = await puppeteer.connect({ browserURL: `http://127.0.0.1:${PORT}` }).catch(() => null);
}
if (!browser) {
  console.error(`The browser did not open its debugging port (${PORT}).`);
  process.exit(1);
}
try {
  for (const name of wanted.length ? wanted : Object.keys(CLIPS)) {
    console.log(name);
    await CLIPS[name](browser);
  }
} finally {
  await browser.close();
}
