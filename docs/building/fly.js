// Fly mode: free flight, no collision, no gravity. Loaded on first use (app.loadFly()), like walk mode.
//
// API (app.fly; the usual way in is app.camera.setMode('fly'), see viewport.js; event 'mode' {mode: 'fly'}):
//   enter({position: [x, y, z], yaw, pitch})   start flying from the current camera, or from the given pose (yaw: 0 looks along +y, turning
//                                              counter-clockwise seen from above; pitch: up is positive)
//   leave()          back to the orbit camera, looking at the house from where the flight ended
//   info()           {active, position: [x, y, z], yaw, pitch, speed}
//   setSpeed(mps)    metres per second, 0.5 to 60 (the wheel changes it)
//   move(dx, dy, dz) a step in metres in the view frame: right, forward (along the view direction), up
//   look(yaw, pitch) set the direction
// Keys: W A S D or the arrows move along the view direction and sideways, Space or E up, Ctrl or Q down, Shift is four times faster, the
// wheel changes the speed, the mouse looks (pointer lock; where that is refused, drag). Touch: the stick at the left moves, the slider at the
// right goes up and down, drag on the rest of the view looks. It flies through walls and below the ground: that is how the foundations are seen.
import * as THREE from 'three';
import { app } from './app.js';
import { rect3d } from './shared.js';

const F = { active: false, yaw: 0, pitch: 0, speed: 4, locked: false };
const keys = new Set(), stick = { x: 0, y: 0, id: null }, slide = { v: 0 };
let ui = null, saved = null, hintAt = 0;
const clamp = (v, a, b) => Math.min(b, Math.max(a, v));
const isTyping = () => { const a = document.activeElement; return a && /^(INPUT|TEXTAREA|SELECT)$/.test(a.tagName); };
const coarse = () => matchMedia('(pointer:coarse)').matches;

function place() { const c = app.camera; c.rotation.order = 'ZXY'; c.rotation.set(Math.PI / 2 + F.pitch, 0, F.yaw); c.updateMatrixWorld(true); }
const forward = () => new THREE.Vector3(-Math.sin(F.yaw) * Math.cos(F.pitch), Math.cos(F.yaw) * Math.cos(F.pitch), Math.sin(F.pitch));
function bound() { const P = app.plot || { x0: -20, x1: 30, y0: -20, y1: 30 }, c = app.camera.position; c.x = clamp(c.x, P.x0 - 25, P.x1 + 25); c.y = clamp(c.y, P.y0 - 25, P.y1 + 25); c.z = clamp(c.z, -25, 70); }
export function info() { const p = app.camera.position; return { active: F.active, position: [+p.x.toFixed(2), +p.y.toFixed(2), +p.z.toFixed(2)], yaw: +F.yaw.toFixed(3), pitch: +F.pitch.toFixed(3), speed: +F.speed.toFixed(2) }; }
export function look(yaw, pitch) { F.yaw = yaw; F.pitch = clamp(pitch, -1.5, 1.5); place(); app.requestRender('3d'); }
export function setSpeed(v) { F.speed = clamp(+v || 4, 0.5, 60); if (ui) ui.speed.textContent = F.speed.toFixed(1) + ' m/s'; return F.speed; }
export function move(dx, dy, dz) {
  const f = forward(), r = new THREE.Vector3(Math.cos(F.yaw), Math.sin(F.yaw), 0);
  app.camera.position.addScaledVector(r, dx).addScaledVector(f, dy).addScaledVector(new THREE.Vector3(0, 0, 1), dz); bound(); place(); app.requestRender('3d');
}
export function enter(o = {}) {
  if (F.active) { if (o.position) app.camera.position.set(...o.position); if (o.yaw != null) F.yaw = o.yaw; if (o.pitch != null) F.pitch = clamp(o.pitch, -1.5, 1.5); place(); app.requestRender('3d'); return info(); }
  const c = app.camera; saved = { fov: c.fov, target: app.controls.target.clone() };
  if (o.position) c.position.set(...o.position);
  const fw = new THREE.Vector3(0, 0, -1).applyQuaternion(c.quaternion);   // the direction the camera looks in now
  F.yaw = o.yaw != null ? o.yaw : Math.atan2(-fw.x, fw.y); F.pitch = o.pitch != null ? clamp(o.pitch, -1.5, 1.5) : Math.asin(clamp(fw.z, -1, 1));
  F.active = true; c.fov = 60; c.updateProjectionMatrix(); c.up.set(0, 0, 1);
  app.mode = 'fly'; app.controls.enabled = false; place(); bound();
  document.body.classList.add('flying'); ui.root.style.display = ''; ui.speed.textContent = F.speed.toFixed(1) + ' m/s'; ui.stick.style.display = coarse() ? 'block' : 'none'; ui.slider.style.display = coarse() ? 'block' : 'none';
  if (!coarse()) { try { app.renderer.domElement.requestPointerLock?.(); } catch { /* not allowed: drag looks */ } }
  app.addTicker(tick, { scope: '3d' }); app.emit('mode', { mode: 'fly' }); return info();
}
export function leave(o = {}) {
  if (!F.active) return info();
  F.active = false; if (document.pointerLockElement) try { document.exitPointerLock(); } catch { /* ignore */ }
  const c = app.camera; c.fov = saved.fov; c.updateProjectionMatrix();
  const f = forward(), d = app.houseBox ? clamp(c.position.distanceTo(app.houseBox.getCenter(new THREE.Vector3())), 3, 40) : 8;   // orbit around a point ahead of the camera
  app.controls.target.copy(c.position).addScaledVector(f, Math.min(d, 15)); if (app.controls.target.z < 0) app.controls.target.z = 0.5; c.lookAt(app.controls.target);
  document.body.classList.remove('flying'); ui.root.style.display = 'none'; keys.clear(); stick.x = stick.y = 0; slide.v = 0;
  if (!o.silent) { app.mode = 'orbit'; app.controls.enabled = true; app.controls.update(); app.emit('mode', { mode: 'orbit' }); }
  app.requestRender(); return info();
}
function tick(dt) {
  if (!F.active) return false;
  const fwd = (keys.has('w') || keys.has('arrowup') ? 1 : 0) - (keys.has('s') || keys.has('arrowdown') ? 1 : 0) - stick.y;
  const side = (keys.has('d') || keys.has('arrowright') ? 1 : 0) - (keys.has('a') || keys.has('arrowleft') ? 1 : 0) + stick.x;
  const up = (keys.has(' ') || keys.has('e') ? 1 : 0) - (keys.has('control') || keys.has('q') ? 1 : 0) + slide.v;
  const m = Math.hypot(fwd, side, up);
  if (m > 0.01) { const v = F.speed * (keys.has('shift') ? 4 : 1) * dt / Math.max(1, m); move(side * v, fwd * v, up * v); }
  const now = performance.now(); if (now - hintAt > 200) { hintAt = now; const p = app.camera.position; ui.pos.textContent = `x ${p.x.toFixed(1)}  y ${p.y.toFixed(1)}  z ${p.z.toFixed(1)}`; }
  return true;
}
export function initFly() {
  if (app.fly) return app.fly;
  const stage = document.getElementById('stage') || app.renderer.domElement.parentElement, cv = app.renderer.domElement;
  const root = document.createElement('div'); root.style.cssText = 'position:absolute;inset:0;pointer-events:none;z-index:6;display:none;font:12px system-ui,sans-serif;color:#e6edf7';
  root.innerHTML = `<div class="fy-help" style="position:absolute;left:50%;bottom:12px;transform:translateX(-50%);background:rgba(10,16,26,.7);padding:4px 10px;border-radius:6px;white-space:nowrap;max-width:94%;overflow:hidden;text-overflow:ellipsis">${coarse() ? 'Left stick moves · right slider up and down · drag to look' : 'W A S D move · Space / E up · Ctrl / Q down · Shift fast · wheel speed · Esc leave'}</div>
    <div class="fy-info" style="position:absolute;left:12px;top:36px;background:rgba(10,16,26,.7);padding:3px 8px;border-radius:6px"><b>Fly</b> <span class="fy-speed"></span> · <span class="fy-pos"></span></div>
    <button class="fy-exit" style="position:absolute;right:12px;top:36px;pointer-events:auto;border:1px solid #2b3a55;background:#111a2b;color:#e6edf7;border-radius:6px;padding:4px 10px;font:600 12px system-ui;cursor:pointer">Leave</button>
    <div class="fy-stick" style="position:absolute;left:24px;bottom:56px;width:96px;height:96px;border-radius:50%;border:2px solid rgba(255,255,255,.35);pointer-events:auto;display:none;touch-action:none"><i style="position:absolute;left:30px;top:30px;width:32px;height:32px;border-radius:50%;background:rgba(255,255,255,.45)"></i></div>
    <input class="fy-slider" type="range" min="-1" max="1" step="0.01" value="0" style="position:absolute;right:14px;bottom:80px;width:120px;transform:rotate(-90deg);transform-origin:right bottom;pointer-events:auto;display:none;touch-action:none">`;
  stage.appendChild(root);
  ui = { root, speed: root.querySelector('.fy-speed'), pos: root.querySelector('.fy-pos'), stick: root.querySelector('.fy-stick'), slider: root.querySelector('.fy-slider') };
  ui.stick.addEventListener('pointerdown', e => { stick.id = e.pointerId; stick.ox = e.clientX; stick.oy = e.clientY; ui.stick.setPointerCapture(e.pointerId); e.stopPropagation(); });
  ui.stick.addEventListener('pointermove', e => { if (stick.id !== e.pointerId) return; const dx = e.clientX - stick.ox, dy = e.clientY - stick.oy, m = Math.min(1, Math.hypot(dx, dy) / 40), a = Math.atan2(dy, dx); stick.x = Math.cos(a) * m; stick.y = Math.sin(a) * m; ui.stick.firstElementChild.style.transform = `translate(${Math.cos(a) * m * 30}px,${Math.sin(a) * m * 30}px)`; });
  const rel = e => { if (stick.id === e.pointerId) { stick.id = null; stick.x = stick.y = 0; ui.stick.firstElementChild.style.transform = ''; } };
  ui.stick.addEventListener('pointerup', rel); ui.stick.addEventListener('pointercancel', rel);
  ui.slider.addEventListener('input', () => { slide.v = +ui.slider.value; }); const zero = () => { ui.slider.value = 0; slide.v = 0; }; ui.slider.addEventListener('pointerup', zero); ui.slider.addEventListener('pointercancel', zero);
  root.querySelector('.fy-exit').onclick = () => leave();
  addEventListener('keydown', e => {
    if (!F.active || isTyping() || e.metaKey || e.altKey) return; const k = e.key.toLowerCase();
    if (k === 'escape') { leave(); return; }
    if (['w', 'a', 's', 'd', 'e', 'q', ' ', 'arrowup', 'arrowdown', 'arrowleft', 'arrowright', 'shift', 'control'].includes(k) && !(e.ctrlKey && k.length === 1)) { keys.add(k); e.preventDefault(); }
  });
  addEventListener('keyup', e => keys.delete(e.key.toLowerCase())); addEventListener('blur', () => { keys.clear(); stick.x = stick.y = 0; });
  document.addEventListener('pointerlockchange', () => { F.locked = document.pointerLockElement === cv; });
  document.addEventListener('mousemove', e => { if (!F.active || !F.locked) return; look(F.yaw - e.movementX * 0.0022, F.pitch - e.movementY * 0.0022); });
  let drag = null;
  cv.addEventListener('pointerdown', e => { if (!F.active || F.locked) return; const r = rect3d(cv); if (e.clientX < r.left || e.clientX > r.right || e.clientY < r.top || e.clientY > r.bottom) return; drag = { id: e.pointerId, x: e.clientX, y: e.clientY }; cv.setPointerCapture(e.pointerId); });
  cv.addEventListener('pointermove', e => { if (!F.active || !drag || drag.id !== e.pointerId) return; const s = e.pointerType === 'touch' ? 0.005 : 0.004; look(F.yaw - (e.clientX - drag.x) * s, F.pitch - (e.clientY - drag.y) * s); drag.x = e.clientX; drag.y = e.clientY; });
  const up = e => { if (drag && drag.id === e.pointerId) drag = null; };
  cv.addEventListener('pointerup', up); cv.addEventListener('pointercancel', up);
  cv.addEventListener('wheel', e => { if (!F.active) return; e.preventDefault(); e.stopPropagation(); setSpeed(F.speed * Math.exp(-e.deltaY * 0.0012)); }, { passive: false, capture: true });
  app.fly = { enter, leave, info, setSpeed, move, look, get active() { return F.active; } };
  return app.fly;
}
