// The hologram: a vehicle as a wireframe of light, with the area a fault is in
// lit up inside it.
//
// # How it is drawn
//
// The body is a mesh of cross-section rings (see `vehicles.ts`). It is drawn
// three times. An invisible copy of its surface writes depth only, so the near
// side hides the far side. The wireframe is then drawn once faintly with no
// depth test, which is the far side showing through, and once brightly with
// it, which is the near side. That is the whole of the "x-ray" look, and it
// needs no shader.
//
// Inside the shell are the engine, gearbox, exhaust, tank, battery, seats and
// brakes, drawn faintly so the body reads as something with a vehicle in it.
// The group a finding belongs to is drawn hot and pulsing, with a beacon on
// the kind of part where one is named.
//
// # What it costs
//
// One WebGL context, about ten thousand line segments and a bloom pass. It
// stops drawing when it is off screen or the window is hidden, because a pane
// that is not being looked at should not keep a laptop's fan on, and the app
// keeps its tabs mounted.

import * as THREE from "three";
import { OrbitControls } from "three/examples/jsm/controls/OrbitControls.js";
import { EffectComposer } from "three/examples/jsm/postprocessing/EffectComposer.js";
import { OutputPass } from "three/examples/jsm/postprocessing/OutputPass.js";
import { RenderPass } from "three/examples/jsm/postprocessing/RenderPass.js";
import { UnrealBloomPass } from "three/examples/jsm/postprocessing/UnrealBloomPass.js";
import type { Region } from "../api/types";
import { Body, RING, buildInternals, ringIndex, ringSpan, type BodyShape } from "./vehicles";

const COOL = 0x35b8ff;
const COOL_BRIGHT = 0xbfefff;
const HOT = 0xff8a1f;
const HOT_BRIGHT = 0xffd9a8;
const BACKDROP = 0x050c1a;

/** Dashes round each turning ring of the platform. */
const DASHES = 36;

/** Stations along the body. Enough that a windscreen is a curve, not steps. */
const STATIONS = 84;

export interface HologramOptions {
  shape: BodyShape;
  region: Region;
  /** The kind of part to point at within the region, when one is named. */
  partId: string | null;
  /** Still picture: no turning, no sweep, no pulse. */
  still: boolean;
  /**
   * Draw only when `step` is called, at the time it is given. For recording:
   * a frame is then the same frame every time it is asked for, and a clip
   * that ends where it began loops without a seam.
   */
  manual?: boolean;
  /** Where the beacon is on screen, in CSS pixels, each frame. */
  onAnchor: (at: { x: number; y: number } | null) => void;
}

export interface Hologram {
  setShape(shape: BodyShape): void;
  setFocus(region: Region, partId: string | null): void;
  /**
   * Draw one frame as it would be `seconds` in, turned `turn` radians from
   * where it started. Only for a hologram made with `manual`.
   */
  step(seconds: number, turn: number): void;
  dispose(): void;
}

/**
 * How long everything that moves takes to come back to where it started, in
 * seconds. The sweep, the pulse, the spreading ring and the turning dashes
 * all divide it exactly, so a recording this long loops.
 */
export const LOOP_SECONDS = 6;

function lineMaterial(color: number, opacity: number, depthTest: boolean): THREE.LineBasicMaterial {
  return new THREE.LineBasicMaterial({
    color,
    transparent: true,
    opacity,
    depthTest,
    depthWrite: false,
    blending: THREE.AdditiveBlending,
  });
}

function segments(points: THREE.Vector3[][]): THREE.BufferGeometry {
  const pairs: THREE.Vector3[] = [];
  for (const line of points) {
    for (let i = 1; i < line.length; i++) pairs.push(line[i - 1], line[i]);
  }
  return new THREE.BufferGeometry().setFromPoints(pairs);
}

/** A soft round glow, or a ring, as a texture. */
function glowTexture(ring: boolean): THREE.CanvasTexture {
  const size = 128;
  const canvas = document.createElement("canvas");
  canvas.width = canvas.height = size;
  const ctx = canvas.getContext("2d")!;
  const g = ctx.createRadialGradient(size / 2, size / 2, 0, size / 2, size / 2, size / 2);
  if (ring) {
    g.addColorStop(0, "rgba(255,255,255,0)");
    g.addColorStop(0.62, "rgba(255,255,255,0)");
    g.addColorStop(0.78, "rgba(255,255,255,1)");
    g.addColorStop(0.9, "rgba(255,255,255,0.15)");
    g.addColorStop(1, "rgba(255,255,255,0)");
  } else {
    g.addColorStop(0, "rgba(255,255,255,1)");
    g.addColorStop(0.25, "rgba(255,255,255,0.55)");
    g.addColorStop(0.6, "rgba(255,255,255,0.12)");
    g.addColorStop(1, "rgba(255,255,255,0)");
  }
  ctx.fillStyle = g;
  ctx.fillRect(0, 0, size, size);
  const texture = new THREE.CanvasTexture(canvas);
  texture.colorSpace = THREE.SRGBColorSpace;
  return texture;
}

/** Everything that makes up one body shape, so a change of shape can drop it. */
interface Built {
  group: THREE.Group;
  body: Body;
  /** The inner groups by area, as [faint lines, hot lines]. */
  areas: Map<string, THREE.LineSegments>;
  anchors: Record<string, THREE.Vector3>;
  scan: THREE.LineLoop;
}

export function createHologram(container: HTMLElement, options: HologramOptions): Hologram {
  const renderer = new THREE.WebGLRenderer({ antialias: true, powerPreference: "low-power" });
  renderer.setPixelRatio(Math.min(window.devicePixelRatio || 1, 2));
  renderer.domElement.className = "holo-canvas";
  container.appendChild(renderer.domElement);

  const scene = new THREE.Scene();
  // The backdrop is the scene's, not the renderer's clear colour. A clear
  // colour is converted for the screen when it is set, then used as it stands
  // on the off-screen buffer the glow is made from, and is converted again on
  // the way out: near-black arrives as slate grey.
  scene.background = new THREE.Color(BACKDROP);
  scene.fog = new THREE.Fog(BACKDROP, 9, 22);
  const camera = new THREE.PerspectiveCamera(30, 2, 0.1, 60);

  const controls = new OrbitControls(camera, renderer.domElement);
  controls.enableZoom = false;
  controls.enablePan = false;
  controls.enableDamping = true;
  controls.dampingFactor = 0.08;
  controls.minPolarAngle = Math.PI * 0.22;
  controls.maxPolarAngle = Math.PI * 0.5;
  controls.autoRotate = !options.still && !options.manual;
  controls.autoRotateSpeed = 0.55;

  const composer = new EffectComposer(renderer);
  composer.addPass(new RenderPass(scene, camera));
  const bloom = new UnrealBloomPass(new THREE.Vector2(256, 256), 0.5, 0.0, 0.42);
  // The pass blurs at five sizes and adds them all. The two widest spread the
  // brightest points over the whole picture as a grey haze, which is the
  // opposite of light on black. Only the glow close to each line is kept.
  bloom.compositeMaterial.uniforms["bloomFactors"].value = [1, 0.75, 0.4, 0.06, 0];
  composer.addPass(bloom);
  composer.addPass(new OutputPass());

  // Shared materials. Everything is lines of light, added together.
  const far = lineMaterial(COOL, 0.07, false);
  const near = lineMaterial(COOL, 0.36, true);
  const creaseFar = lineMaterial(COOL_BRIGHT, 0.1, false);
  const creaseNear = lineMaterial(COOL_BRIGHT, 0.8, true);
  const inner = lineMaterial(COOL, 0.26, false);
  const hot = lineMaterial(HOT, 1, false);
  const scanLine = lineMaterial(0xffffff, 0.9, false);
  const depthOnly = new THREE.MeshBasicMaterial({
    colorWrite: false,
    side: THREE.DoubleSide,
    polygonOffset: true,
    polygonOffsetFactor: 1.5,
    polygonOffsetUnits: 2,
  });
  const skin = new THREE.MeshBasicMaterial({
    color: 0x0b4f86,
    transparent: true,
    opacity: 0.1,
    depthWrite: false,
    blending: THREE.AdditiveBlending,
    side: THREE.FrontSide,
  });
  const glow = glowTexture(false);
  const ringGlow = glowTexture(true);
  const disposables: { dispose(): void }[] = [
    far, near, creaseFar, creaseNear, inner, hot, scanLine, depthOnly, skin, glow, ringGlow,
  ];

  // The beacon on the part, and the ring that spreads from it.
  const beacon = new THREE.Sprite(
    new THREE.SpriteMaterial({ map: glow, color: HOT_BRIGHT, blending: THREE.AdditiveBlending, depthTest: false, transparent: true }),
  );
  const ping = new THREE.Sprite(
    new THREE.SpriteMaterial({ map: ringGlow, color: HOT, blending: THREE.AdditiveBlending, depthTest: false, transparent: true }),
  );
  beacon.renderOrder = 10;
  ping.renderOrder = 10;
  scene.add(beacon, ping);
  disposables.push(beacon.material, ping.material);

  // ---------------------------------------------------------------- platform
  const platform = new THREE.Group();
  const spinner = new THREE.Group();
  scene.add(platform);
  platform.add(spinner);
  const platformParts: (THREE.Line | THREE.Mesh)[] = [];

  function buildPlatform(radius: number) {
    for (const part of platformParts) {
      part.geometry.dispose();
      part.parent?.remove(part);
    }
    platformParts.length = 0;
    const loop = (r: number, material: THREE.Material) => {
      const points: THREE.Vector3[] = [];
      for (let i = 0; i <= 96; i++) {
        const a = (i / 96) * Math.PI * 2;
        points.push(new THREE.Vector3(Math.cos(a) * r, 0, Math.sin(a) * r));
      }
      const line = new THREE.Line(new THREE.BufferGeometry().setFromPoints(points), material);
      platform.add(line);
      platformParts.push(line);
    };
    // Dashes drawn one by one, `DASHES` to a ring whatever its size, so the
    // ring looks the same after turning by one dash.
    const dashes = (r: number) => {
      const arcs: THREE.Vector3[][] = [];
      for (let i = 0; i < DASHES; i++) {
        const arc: THREE.Vector3[] = [];
        for (let k = 0; k <= 4; k++) {
          const a = ((i + (k / 4) * 0.58) / DASHES) * Math.PI * 2;
          arc.push(new THREE.Vector3(Math.cos(a) * r, 0, Math.sin(a) * r));
        }
        arcs.push(arc);
      }
      const lines = new THREE.LineSegments(segments(arcs), platformDash);
      spinner.add(lines);
      platformParts.push(lines);
    };
    loop(radius, platformBright);
    loop(radius * 0.86, platformDim);
    loop(radius * 0.6, platformDim);
    dashes(radius * 1.08);
    dashes(radius * 0.73);

    const ticks: THREE.Vector3[][] = [];
    for (let i = 0; i < 72; i++) {
      const a = (i / 72) * Math.PI * 2;
      const long = i % 6 === 0;
      const r0 = radius * (long ? 0.9 : 0.95);
      ticks.push([
        new THREE.Vector3(Math.cos(a) * r0, 0, Math.sin(a) * r0),
        new THREE.Vector3(Math.cos(a) * radius, 0, Math.sin(a) * radius),
      ]);
    }
    const tickLines = new THREE.LineSegments(segments(ticks), platformDim);
    platform.add(tickLines);
    platformParts.push(tickLines);

    const disc = new THREE.Mesh(new THREE.CircleGeometry(radius * 1.25, 48), platformGlow);
    disc.rotation.x = -Math.PI / 2;
    disc.position.y = -0.005;
    platform.add(disc);
    platformParts.push(disc);
  }
  const platformBright = lineMaterial(COOL_BRIGHT, 0.55, true);
  const platformDim = lineMaterial(COOL, 0.32, true);
  const platformDash = lineMaterial(COOL, 0.6, true);
  const platformGlow = new THREE.MeshBasicMaterial({
    map: glow,
    color: 0x1479c4,
    transparent: true,
    opacity: 0.5,
    depthWrite: false,
    blending: THREE.AdditiveBlending,
  });
  disposables.push(platformBright, platformDim, platformDash, platformGlow);

  // -------------------------------------------------------------------- body
  let built: Built | null = null;
  let focus: { region: Region; partId: string | null } = { region: options.region, partId: options.partId };

  function build(shape: BodyShape): Built {
    const body = new Body(shape);
    const { spec } = body;
    const group = new THREE.Group();
    const rings: THREE.Vector3[][] = [];
    for (let i = 0; i < STATIONS; i++) rings.push(body.ringAt(i / (STATIONS - 1)));

    // The surface, for depth and a faint skin.
    const positions: number[] = [];
    for (const ring of rings) for (const p of ring) positions.push(p.x, p.y, p.z);
    const index: number[] = [];
    for (let i = 0; i < STATIONS - 1; i++) {
      for (let j = 0; j < RING; j++) {
        const a = i * RING + j;
        const b = i * RING + ((j + 1) % RING);
        const c = a + RING;
        const d = b + RING;
        index.push(a, c, b, b, c, d);
      }
    }
    // Close the nose and the tail.
    for (const end of [0, STATIONS - 1]) {
      const ring = rings[end];
      const centre = ring.reduce((sum, p) => sum.add(p), new THREE.Vector3()).multiplyScalar(1 / RING);
      const hub = positions.length / 3;
      positions.push(centre.x, centre.y, centre.z);
      for (let j = 0; j < RING; j++) index.push(hub, end * RING + j, end * RING + ((j + 1) % RING));
    }
    const surface = new THREE.BufferGeometry();
    surface.setAttribute("position", new THREE.Float32BufferAttribute(positions, 3));
    surface.setIndex(index);
    surface.computeVertexNormals();
    const depth = new THREE.Mesh(surface, depthOnly);
    depth.renderOrder = 0;
    const shell = new THREE.Mesh(surface, skin);
    shell.renderOrder = 1;
    group.add(depth, shell);

    // The mesh of lines: every other ring, and every other line along.
    const mesh: THREE.Vector3[][] = [];
    for (let i = 0; i < STATIONS; i += 2) mesh.push([...rings[i], rings[i][0]]);
    for (let j = 0; j < RING; j += 2) mesh.push(rings.map((ring) => ring[j]));
    const meshGeometry = segments(mesh);

    // The creases a person recognises a vehicle by.
    const crease: THREE.Vector3[][] = [];
    const along = (control: number, mirror: boolean, keep: (t: number) => boolean) => {
      let run: THREE.Vector3[] = [];
      rings.forEach((ring, i) => {
        if (keep(i / (STATIONS - 1))) run.push(ring[ringIndex(control, mirror)]);
        else if (run.length) {
          crease.push(run);
          run = [];
        }
      });
      if (run.length) crease.push(run);
    };
    const ringNear = (t: number) => rings[Math.round(t * (STATIONS - 1))];
    for (const mirror of [false, true]) {
      along(5, mirror, () => true); // waistline
      along(2, mirror, () => true); // sill, which follows the wheel arches
      along(6, mirror, (t) => body.cabin(t) > 0.5); // bottom of the glass
      along(7, mirror, (t) => body.cabin(t) > 0.5); // roof rail
      for (const t of spec.pillars) crease.push(ringSpan(ringNear(t), 6, 7, mirror));
      for (const t of spec.doors) crease.push(ringSpan(ringNear(t), 2, 5, mirror));
    }
    // Across the top: the edges of the windscreen and the back glass.
    for (const t of spec.glassEdges) crease.push(ringSpan(ringNear(t), 6, 12));
    // The nose and the tail, and bars across each: a grille, a tailgate.
    for (const end of [rings[0], rings[STATIONS - 1]]) {
      crease.push([...end, end[0]]);
      for (const control of [2, 3, 4, 5]) {
        crease.push([end[ringIndex(control)], end[ringIndex(control, true)]]);
      }
    }
    if (spec.bed) {
      crease.push(ringSpan(ringNear(spec.bed.from + 0.004), 5, 13));
      crease.push(ringSpan(ringNear(spec.bed.to - 0.004), 5, 13));
    }
    const creaseGeometry = segments(crease);

    const meshFar = new THREE.LineSegments(meshGeometry, far);
    const meshNear = new THREE.LineSegments(meshGeometry, near);
    const creaseBack = new THREE.LineSegments(creaseGeometry, creaseFar);
    const creaseFront = new THREE.LineSegments(creaseGeometry, creaseNear);
    for (const [line, order] of [[meshFar, 2], [meshNear, 3], [creaseBack, 2], [creaseFront, 4]] as const) {
      line.renderOrder = order;
      group.add(line);
    }

    // Wheels: two rims, spokes and a tread, and a drum that hides what is behind.
    const r = spec.wheelRadius;
    const track = spec.halfWidth - spec.tireWidth / 2 - 0.03;
    const wheelLines: THREE.Vector3[][] = [];
    for (const axle of body.axles) {
      for (const side of [-1, 1]) {
        const z = side * track;
        const faces = [z - spec.tireWidth / 2, z + spec.tireWidth / 2];
        for (const face of faces) {
          for (const radius of [r, r * 0.66]) {
            const rim: THREE.Vector3[] = [];
            for (let i = 0; i <= 32; i++) {
              const a = (i / 32) * Math.PI * 2;
              rim.push(new THREE.Vector3(axle + Math.cos(a) * radius, r + Math.sin(a) * radius, face));
            }
            wheelLines.push(rim);
          }
        }
        const outer = side < 0 ? faces[0] : faces[1];
        for (let i = 0; i < 10; i++) {
          const a = (i / 10) * Math.PI * 2;
          const c = Math.cos(a);
          const s = Math.sin(a);
          wheelLines.push([
            new THREE.Vector3(axle + c * r * 0.14, r + s * r * 0.14, outer),
            new THREE.Vector3(axle + c * r * 0.66, r + s * r * 0.66, outer),
          ]);
          wheelLines.push([
            new THREE.Vector3(axle + c * r, r + s * r, faces[0]),
            new THREE.Vector3(axle + c * r, r + s * r, faces[1]),
          ]);
        }
        const drum = new THREE.Mesh(new THREE.CylinderGeometry(r, r, spec.tireWidth, 24), depthOnly);
        drum.rotation.x = Math.PI / 2;
        drum.position.set(axle, r, z);
        group.add(drum);
      }
    }
    const wheelGeometry = segments(wheelLines);
    const wheelBack = new THREE.LineSegments(wheelGeometry, creaseFar);
    const wheelFront = new THREE.LineSegments(wheelGeometry, creaseNear);
    wheelBack.renderOrder = 2;
    wheelFront.renderOrder = 4;
    group.add(wheelBack, wheelFront);

    // Lamps: a glow at each corner, white ahead and red behind.
    const lamp = (t: number, colour: number, size: number) => {
      const y = body.deck(t) - 0.14;
      for (const side of [-1, 1]) {
        const sprite = new THREE.Sprite(
          new THREE.SpriteMaterial({ map: glow, color: colour, blending: THREE.AdditiveBlending, transparent: true, depthWrite: false }),
        );
        sprite.position.set(body.x(t), y, side * body.halfWidth(t) * 0.82);
        sprite.scale.setScalar(size);
        group.add(sprite);
      }
    };
    lamp(0.006, 0xcfeaff, 0.3);
    lamp(0.996, 0xff3b30, 0.26);

    // What is inside.
    const internals = buildInternals(body);
    const areas = new Map<string, THREE.LineSegments>();
    for (const [name, geometries] of Object.entries(internals.groups)) {
      const merged: THREE.Vector3[] = [];
      for (const geometry of geometries) {
        const position = geometry.getAttribute("position");
        for (let i = 0; i < position.count; i++) {
          merged.push(new THREE.Vector3().fromBufferAttribute(position, i));
        }
        geometry.dispose();
      }
      const lines = new THREE.LineSegments(new THREE.BufferGeometry().setFromPoints(merged), inner);
      lines.renderOrder = 5;
      group.add(lines);
      areas.set(name, lines);
    }

    const scan = new THREE.LineLoop(new THREE.BufferGeometry().setFromPoints(rings[0]), scanLine);
    scan.renderOrder = 6;
    scan.visible = false;
    group.add(scan);

    scene.add(group);
    buildPlatform(spec.length * 0.43);
    return { group, body, areas, anchors: internals.anchors, scan };
  }

  function drop(target: Built) {
    scene.remove(target.group);
    target.group.traverse((object) => {
      const withGeometry = object as THREE.Mesh;
      if (withGeometry.geometry) withGeometry.geometry.dispose();
      if (object instanceof THREE.Sprite) object.material.dispose();
    });
  }

  /** Where the beacon goes: on the part if it is known here, else the area. */
  function anchor(): THREE.Vector3 | null {
    if (!built || focus.region === "unknown") return null;
    return (focus.partId && built.anchors[focus.partId]) || built.anchors[focus.region] || null;
  }

  function applyFocus() {
    if (!built) return;
    for (const [name, lines] of built.areas) {
      const lit = name === focus.region;
      lines.material = lit ? hot : inner;
      lines.renderOrder = lit ? 7 : 5;
    }
    const at = anchor();
    beacon.visible = ping.visible = !!at;
    if (at) {
      beacon.position.copy(at);
      ping.position.copy(at);
    }
  }

  function frame() {
    const { length } = built!.body.spec;
    const height = built!.body.roof(0.5);
    const aspect = camera.aspect;
    const half = THREE.MathUtils.degToRad(camera.fov / 2);
    // Far enough back that the whole length fits whichever way it is turned.
    const forWidth = (length * 0.6) / (Math.tan(half) * aspect);
    const forHeight = (height * 1.15) / Math.tan(half);
    const distance = Math.max(forWidth, forHeight);
    const target = new THREE.Vector3(0, height * 0.36, 0);
    const azimuth = controls.getAzimuthalAngle();
    const polar = controls.getPolarAngle();
    controls.target.copy(target);
    camera.position.set(
      target.x + distance * Math.sin(polar) * Math.sin(azimuth),
      target.y + distance * Math.cos(polar),
      target.z + distance * Math.sin(polar) * Math.cos(azimuth),
    );
    controls.update();
  }

  // Start three-quarters on, nose towards the viewer's left.
  camera.position.set(6.6, 2.3, -4.6);
  controls.target.set(0, 0.5, 0);
  controls.update();

  built = build(options.shape);
  applyFocus();

  function resize() {
    const width = Math.max(1, container.clientWidth);
    const height = Math.max(1, container.clientHeight);
    renderer.setSize(width, height, false);
    composer.setSize(width, height);
    camera.aspect = width / height;
    camera.updateProjectionMatrix();
    frame();
  }
  const resizer = new ResizeObserver(resize);
  resizer.observe(container);
  resize();

  /** Where a recording's view started, so each frame is turned from there. */
  let startAzimuth: number | null = null;

  // Only draw while somebody can see it.
  let visible = true;
  let raf = 0;
  const projected = new THREE.Vector3();
  const clock = new THREE.Clock();

  /** Everything that moves, put where it is `now` seconds in, and drawn. */
  function draw(now: number) {
    if (!built) return;
    if (!options.still) {
      // One dash a second, so six seconds on the rings are where they began.
      spinner.rotation.y = (now * Math.PI * 2) / DASHES;
      // The sweep: one cross-section of light travelling nose to tail.
      const cycle = (now % LOOP_SECONDS) / LOOP_SECONDS;
      const sweeping = cycle < 0.55;
      built.scan.visible = sweeping;
      if (sweeping) built.scan.geometry.setFromPoints(built.body.ringAt(cycle / 0.55));
      const beat = 0.5 + 0.5 * Math.sin(now * Math.PI);
      hot.opacity = 0.62 + 0.38 * beat;
      const spread = (now / 1.5) % 1;
      const size = built.body.spec.length * 0.028;
      beacon.scale.setScalar(size * (2.4 + beat * 0.9));
      ping.scale.setScalar(size * (2 + spread * 7));
      ping.material.opacity = 1 - spread;
    } else {
      const size = built.body.spec.length * 0.028;
      beacon.scale.setScalar(size * 2.8);
      ping.scale.setScalar(size * 5.5);
      ping.material.opacity = 0.7;
    }

    composer.render();

    const at = anchor();
    if (at) {
      projected.copy(at).project(camera);
      options.onAnchor({
        x: (projected.x * 0.5 + 0.5) * container.clientWidth,
        y: (-projected.y * 0.5 + 0.5) * container.clientHeight,
      });
    } else {
      options.onAnchor(null);
    }
  }

  function render() {
    raf = 0;
    if (options.manual || !visible || document.hidden || !built) return;
    controls.update();
    draw(clock.getElapsedTime());
    raf = requestAnimationFrame(render);
  }
  const wake = () => {
    if (!raf && !options.manual) raf = requestAnimationFrame(render);
  };
  const watcher = new IntersectionObserver((entries) => {
    visible = entries.some((entry) => entry.isIntersecting);
    if (visible) wake();
  });
  watcher.observe(container);
  document.addEventListener("visibilitychange", wake);
  // Somebody turning it by hand stops it turning by itself, for good: they
  // have put it where they want to look at it.
  const stopTurning = () => {
    controls.autoRotate = false;
  };
  controls.addEventListener("start", stopTurning);
  wake();

  return {
    setShape(shape) {
      if (built?.body.shape === shape) return;
      if (built) drop(built);
      built = build(shape);
      applyFocus();
      frame();
      wake();
    },
    setFocus(region, partId) {
      focus = { region, partId };
      applyFocus();
      wake();
    },
    step(seconds, turn) {
      // A hologram on a tab that is not showing has no size and nothing to
      // draw on. The app keeps its tabs mounted, so a recording finds those too.
      if (container.clientWidth === 0 || container.clientHeight === 0) return;
      // Turned about the vehicle from where the view started, at the same
      // height and distance.
      if (startAzimuth === null) startAzimuth = controls.getAzimuthalAngle();
      const azimuth = startAzimuth + turn;
      const polar = controls.getPolarAngle();
      const distance = camera.position.distanceTo(controls.target);
      camera.position.set(
        controls.target.x + distance * Math.sin(polar) * Math.sin(azimuth),
        controls.target.y + distance * Math.cos(polar),
        controls.target.z + distance * Math.sin(polar) * Math.cos(azimuth),
      );
      camera.lookAt(controls.target);
      draw(seconds);
    },
    dispose() {
      cancelAnimationFrame(raf);
      watcher.disconnect();
      resizer.disconnect();
      document.removeEventListener("visibilitychange", wake);
      controls.removeEventListener("start", stopTurning);
      controls.dispose();
      if (built) drop(built);
      for (const part of platformParts) part.geometry.dispose();
      for (const item of disposables) item.dispose();
      composer.dispose();
      renderer.dispose();
      renderer.forceContextLoss();
      renderer.domElement.remove();
    },
  };
}
