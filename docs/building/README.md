# FZK-Haus viewer: modules and the app object

`docs/building.html` holds the markup and the CSS (tokens, components, shell layout, one responsive block) and one
`<script type="module" src="building/main.js">`. Everything else is in this folder. All modules share one object, `app`
(`app.js`), also on `window.reteBuilding`. Frame: IFC world, metres, Z up, the same numbers as the GLB and `geo3:x/y/z`. No conversion.

The page is a dashboard: a top bar with five menus (View, Layers, Simulate, Tools, About), a rail of side panels, the viewport
(one WebGL canvas, one or four panes), an inspector on the right, a bottom panel with seven tabs and a status bar. The 3D side
knows nothing about the shell and the shell reaches the 3D side only through `app`. `API-viewport.md` documents the rendering side
(layouts, sections, camera modes, ground cutaway, structure, state variables, rooms style, reset). The header comment of
`graphview.js` documents the graph view.

## Boot order (`main.js`)

1. the renderer and scene (`scene.js`); the index (`fzk-viewer.json`), the systems JSON and the GLB are fetched in parallel;
2. the GLB is decoded, the house is added with one plain material and drawn: **first frame** (`app.timing.firstFrame`);
3. after that frame: textures, doors and windows, batches, the sun, the viewport (`viewport.js`), `ui.js`, then the shell
   (`shell-init.js`: dashboard, status bar, console, logs, graph tab, validate, actions, viewport controls). The shell's modules
   are imported in parallel with the model and are not needed for the first frame; if they fail to load the house still works.
   `app.interactive` and the `interactive` event come after this (`app.timing.interactive`);
4. the services layer (`systems*.js`, only when the systems JSON loaded), the WebMCP tools, the scenery. `app.ready` and `ready`
   are the end of all of it (`app.timing.ready`).

Lazy: `walk.js` (first use, `app.loadWalk()`), `fly.js` (`app.loadFly()`), `thermal*.js` (`app.loadThermal()`), `scenery.js`,
`graphview.js` (first open of the Graph tab), `zfight.js` (`app.findCoplanar()`), `measure.js`, `about.js`, `validate.js` (engine call).
Without the systems file the page makes no request for it and is the plain house viewer; the shell says what needs the services layer.
An idle page renders nothing (`app.frameCount` does not move).

## Modules

Shell (the dashboard; none of these draws in the 3D scene):

| Module | Owns |
| --- | --- |
| `dashboard.js` | rail, side panels, inspector, bottom tabs, sizes and resizers, top-right controls, theme, density, units, settings; `app.shell` |
| `shell-init.js` | starts the shell after the house is on screen and wires the lazy panels |
| `menus.js` | menus (roles, keys, type-ahead), the command palette |
| `commands.js` | the command table (one table for menus, palette, About and keys) and the keyboard handler |
| `actions.js` | the commands, the content of the five menus, palette sources, reset with undo, the shareable link, exports; `app.commands`, `app.link`, `app.resetShell` |
| `statusbar.js` | the status bar, `app.notify(message, {ms})` |
| `logs.js` | the log (`app.log(level, source, message, data)`, `app.logs`) and the Logs tab; starts with the page |
| `console.js` | the SPARQL console and the Results table; `app.console`, `app.results` |
| `validate.js` | the Validate tab: SHACL examples from the data, an editor, the engine's report; `app.validate` |
| `graph-tab.js` | the Graph tab: `graphview.js` mounted on first open, wired both ways to the selection, highlights, tints, the "System" grouping; `app.graphTab` |
| `inspector.js` | what is selected: element, room, device, run, structural element, surface |
| `statevars-ui.js` | state variables in panels, generated from their definitions |
| `viewport-ui.js` | controls of the rendering API (structure colours, ground, section pop-over); they are disabled with a tooltip when the API is missing |
| `systems-ui.js`, `thermal-ui.js` | services layers, scenarios, thermal panel and tab |
| `about.js`, `measure.js`, `clock.js` | the About dialog, the measure tool, the clock in the viewport corner |
| `shared.js`, `icons.js` | helpers (`$`, `esc`, `ui` saved state, `fmt` units, `has(path)`, `rect3d`), inline SVG icons |
| `webmcp.js`, `webmcp-viewport.js` | the WebMCP tools: `buildingTools()` (house), `shellTools()`, `systemsTools()`, `perfTools()`, `viewportTools(app)` |

3D side:

| Module | Owns |
| --- | --- |
| `app.js` | the `app` object, event emitter |
| `scene.js` | renderer, camera, orbit controls, lights, sky, ground, quality, interior lights, render loop |
| `viewport.js` | one canvas, panes (single, quad), orthographic panes, picking, camera modes; `section.js` cuts; `fly.js`, `walk.js` |
| `sun.js` | NOAA solar position, site, sunrise/sunset, sun path arc, compass rose, play day |
| `materials.js`, `batch.js` | canvas textures, PBR materials, colour modes, `paint()`; the batches (many elements as a few meshes) |
| `states.js` | doors and windows split into parts on hinges, the state store |
| `rooms.js`, `ground.js`, `structure.js`, `statevars.js`, `reset.js` | room outlines, the trench, the structural rule, state variables, reset scopes |
| `ui.js` | picking, selection, classes, storeys, layers, search, ready queries (`registerLayer`, `registerInfo`, `registerPickable`, `registerSearch`) |
| `queries.js` | ready SPARQL examples, the wasm engine, result conversion |
| `scenery.js`, `zfight.js` | sky dome, ground, trees, birds; the z-fighting diagnostic |
| `fixtures.js`, `systems.js`, `thermal.js` | the parts of the services layer, its model (placement, live state, trace, scenarios, x-ray, isolate), the thermal model (no DOM) |
| `main.js` | boot order, lazy loaders, `SYSTEMS_JSON` / `SYSTEMS_RETE` |

## The shell

`app.shell = { openSide(id), closeSide(), toggleSide(), openInspector(), closeInspector(), openTab(id), closeBottom(), toggleBottom(),
maximiseBottom(on), state(), onSide(id, fn), onTab(id, fn), setBadge(tab, text, hot), setTheme(t), setDensity(d), setUnits(u), openSettings(), notifyResize() }`.
Events: `panel` `{side | inspector}`, `tab` `{tab}`, `theme`, `units`, `shell`, `shellresize`, `command`. The state (which panel, sizes, theme) is kept in
`localStorage` under `reteBuilding.ui.v1`. Widths: at 900 px and under the inspector floats and the side panel gives way to it; at 640 px and under
the rail moves to the bottom, the menus fold into one "Menu" button and panels are sheets.

Everything the user can do is a command or a menu item, the same entry for the menu, the palette and the key, and every call that changes the
page is in the log. The shell uses an API of the rendering side only after `has('x.y')` says it is a function (`shared.js`): the control is
then disabled with a tooltip, never throwing.

### Add a side panel

1. In `building.html` add `<section class="pane" id="pane-NAME" data-pane="NAME" aria-label="..." hidden>` inside `#side` with a `.pane-head` and a `.pane-body`.
2. In `dashboard.js` add `['NAME', 'Title', 'icon']` to `RAIL` (the icon is a name in `icons.js`).
3. Fill it from a module started in `shell-init.js`; `onSide('NAME', fn)` runs when it opens (do lazy work there).

### Add a bottom tab

Add `<div class="tp" id="tp-NAME" role="tabpanel" data-tab="NAME"></div>` to `#btBody`, `['NAME', 'Title']` to `TABS` in `dashboard.js`,
build the content in a module and call `onTab('NAME', fn)`. `setBadge('NAME', text, hot)` puts a count on the tab.

### Add a command, a menu item

`defineCommands([{ id, title, group, keys?, enabled?, run }])` in `actions.js`; `enabled()` returns a reason string to disable the command, anything else
leaves it enabled. Menu items are `{label, run, keys?, check?|radio?, disabled?, keepOpen?}` in `viewMenu`, `layersMenu`, `simulateMenu`, `toolsMenu` or `aboutMenu`. `runCommand(id)` logs it and notifies when it is disabled.

### Add a WebMCP tool

Add `{ name, description, inputSchema, annotations?, execute }` to the array of the module that owns the thing (`buildingTools`, `shellTools`,
`systemsTools`, `perfTools`, `viewportTools`). `webmcp.js` wraps every tool (timing, logging, the status bar pill, `window.reteBuildingTools`) and registers
it with `document.modelContext`; results are JSON of at most about 18 KB, errors are thrown with a message that says what to do. The checks derive
the expected list from those arrays, so a new tool needs no change in a test.

### Add a layer (a group of things drawn together)

`app.registerLayer({ id, label, color, group, count, visible })` adds a toggle row under Services; `group` is a three.js `Group` added to `app.house`.
`app.registerInfo(fn)` adds rows to the inspector of a picked thing, `app.registerPickable(object3D, id, meta)` makes it clickable, `app.registerSearch(fn)`
adds entries to search and the palette, `app.addExamples(title, items)` adds ready queries.

### Add a state variable control or a viewport API

A new API of the rendering side goes on `app` (feature-detect it with `has`) and its controls in `viewport-ui.js`; the shell never imports a rendering module by path.

## Tests

`tests/gate/checks/check_building*.mjs` (five files, run one at a time in the Playwright image; see the header of each): `check_building.mjs` (the house, the
tools, the sun, doors, walk), `check_building_systems.mjs` (services, scenarios, thermal), `check_building_graph.mjs` (the graph view on its harness page),
`check_building_viewport.mjs` (the rendering API), `check_building_dashboard.mjs` (the shell, with `ONLY="menus,logs"` to run some sections). Expectations that depend on
the data (counts, labels, ids) are read from the data at run time; `FZK_SYSTEMS_DIR` routes the two R2 files to a local folder.

## app object

Data (filled before `ready`):
`scene`, `camera`, `renderer`, `controls` (OrbitControls), `house` (Group holding the GLB and room floors/labels),
`graph` (fzk-viewer.json), `nodeByGuid` (guid -> node `{n,l,c,k,st,bd,adj,i,...}`),
`meshByGuid` (guid -> Object3D; a Mesh, or a Group for doors and windows, whose descendant meshes carry `userData.guid`),
`roomByGuid` (guid -> `{guid,label,storey,box:Box3,center:Vector3,floorZ,node}`),
`systems` (parsed systems JSON or `null`), `mode` (`'orbit'|'walk'`), `ready`, `HOUSE_CENTER`, `envTexture`,
`readyPromise` / `whenReady()`, `SYSTEMS_JSON`, `SYSTEMS_RETE`, `examples`, `boundedBy`,
`svc` (the services model, below, or `undefined`), `thermal`, `thermalUi`, `graphSource` (the `.rete` URL the engine really loaded).

Events: `on(event, fn)` returns an unsubscribe function; `off(event, fn)`; `emit(event, payload)`.
- `interactive` the house is drawn and usable; `ready` `{systems, services}` once everything is built (services, tools and scenery too)
- `select` `{id, kind: 'element'|'room'|'pickable'|null, node?, object?, label}`
- `state` `{id, state, prev, kind, label}`
- `mode` `{mode: 'orbit'|'walk'}`
- also: `time` (sun changed; payload is `sun.get()`), `site`, `sunpath`, `play`, `lights` `{on}`, `colourmode` `{mode}`, `quality`, `visibility`, `layer` `{id, visible}`, `result`, `walk`, `interact` `{id}` (walk mode use on a pickable), `render` (after each draw)
- added by the services pass: `svclive` (what is live changed), `xray` `{on}`, `isolate` `{system}`, `trace` (a trace result or `null`), `scenario` (a scenario result or `null`), `thermal` (a run's result), `thermalview` `{temps, sun}`, `resultclear`; `result` now carries `picks` (service ids found in the answer)

Rendering: `requestRender()`; `addBeforeRender(fn)`; `addTicker(fn, {fps})` can cap the rate (between calls it neither runs nor asks for a frame); `renderNow()`; `addTicker(fn)` where `fn(dt, now)` returns `false` to stop (any other
value keeps it and keeps drawing); `isAnimating()`; `frameCount`.

Camera: `focus(object3D | Box3 | array, {snap, distance})`, `frameAll(snap)`, `animateCam(pos, target, ms)`.

Selection and panels:
- `select(guidOrIdOrNode | null, {focus})` selects an element or room by GUID (or unique label), or a registered pickable by id; returns `{id, kind, label}`; throws on an unknown id.
- `clearSelection()`, `selectedInfo()`.
- `registerLayer({id, label, color, group, count, visible}) -> {id, el, group, setCount(n), setVisible(b), isVisible(), remove()}` adds a toggle row under "Layers" in the left panel; the group is added to `house` if it has no parent; emits `layer`.
- `registerInfo(fn) -> unregister`. `fn(thing)` gets `{id, kind: 'element'|'room'|'pickable', node?, object?, meta?, label}` and returns an HTML string, a Node, an array of those, or null. Rows are added to the info panel of the selected thing.
- `registerPickable(object3D, id, meta?) -> unregister`. Makes the object (and its descendants) clickable; click calls `select(id)`. `meta.label`, `meta.kind` show in the info panel. Also usable with the crosshair in walk mode (E selects it and emits `interact`). **New:** `meta.hint` (a string, or a function returning one) is what walk mode shows under the crosshair instead of "Use".
- `registerSearch(fn) -> unregister` **(new)**. `fn(query)` (lower case, never empty) returns `[{label, tag, run}]`; the entries are appended to the search list.
- `addExamples(title, items)` **(new)** adds a titled group of ready queries after the page is built. An item is `{label, desc, q, pre?, cols?, run?}`: `pre` is extra `PREFIX` lines after the common ones; without `run` the query is run by the engine and its answer shown (and highlighted) like any other.
- `setVisibility({classes, storeys, roof, roomNames})`, `getVisibility()`, `showResult({label, cols, rows, idxs, picks?})`, `clearResult()`, `resultPicks`.
- `paint(object3D, {color, opacity, emissive} | null)` tints or restores an element (what selection uses). With the services layer, x-ray wraps it so walls, slabs and the roof stay translucent through selections and results.
- `restyle()`, `refreshInfo()`, `rebuildList()` **(new)**: repaint after selection/result state, redraw the info panel, redraw the search list.

States (`states.js`): `getState(id)`, `setState(id, state, {instant})` (throws on a bad value),
`states(id)` allowed values, `allStates()` `{id: state}`, `stateList()` `[{id,label,kind,state,values}]`,
`toggleState(id)`, `openness(id)` 0..1, `settleStates()` jumps to the end pose,
`registerStateful(id, {values, initial, kind, label, onChange})` for other things (light on/off, valve),
`openables()` the door/window articulations.
Doors: `closed|open`. Windows: `closed|tilted|open`. Persists nothing. The services layer registers every device that has a state in the JSON `states` map (kind = the node type).

Light and look:
- `sun.get()` `{date, time (local hours), timeStr, azimuth, elevation, direction (Vector3, model frame), isDay, sunrise, sunset, solarNoon, ...Str, site}`
- `sun.set({date:'YYYY-MM-DD', time: hours | 'HH:MM'})` local to the site; `sun.position(dateTime, siteOverride?)` is pure (dateTime: Date, ms, ISO string with offset, or `{date, time}`) and returns `{azimuth, elevation, direction, isDay, declination}`; `sun.sunTimes(date)`; `sun.playDay(bool)`; `sun.setPathVisible(bool)`; `sun.site` (a copy of the current site).
- `setSite({latitude, longitude, timezone, trueNorth:[nx,ny] | northAzimuthDeg})`. Defaults: 49.10, 8.43, Europe/Berlin, north = +Y. Taken from `systems.site` when present (the IFC says the house is turned 50 degrees). Also `app.sun.setSite`.
- `getSunDirection()`, `applySun(dir, elevationDeg)` (low-level; `sun.set` calls it).
- `setInteriorLights(on)`, `getInteriorLights()`, `setRoomLights(rooms)`, `interiorLightGroup`. Without the services layer: one point light per room near the ceiling; they follow the sun automatically (on below 4 degrees) until `setInteriorLights` is called by hand. **With it:** `interiorLightGroup` is hidden and `setInteriorLights(on)` switches every indoor light fixture (and the switches that control them) on or off; `getInteriorLights()` is true when any indoor light is on. The sun still turns them on at night and off by day (only when it crosses 4 degrees, so a light you switched stays as it is), and the outdoor lights follow the same rule.
- `setColourMode('realistic'|'class')`, `getColourMode()`; `setQuality('high'|'low')`, `getQuality()` (low = no shadows; default low under 860 px).

Walk: `walk.enter({x,y,storey,yaw,lock})`, `walk.leave()`, `walk.toggle()`, `walk.info()`, `walk.teleport(x,y,{storey,yaw})`,
`walk.step(dx,dy)` (collision move), `walk.blocked(x,y,storey)`, `walk.interact()`, `walk.climb()`, `walk.roomAt()`, `walk.findFree(x,y,storey)`, `walk.active`.
Fixtures do not block the walker.

## Systems file

Fetched once from `SYSTEMS_JSON` (R2; a 404 is normal until it is uploaded). If it has `openings`, the hinge, closed
direction, `opens` sign, `maxAngle` and default `state` of a door or window come from there; if it has `site`, the sun uses it.
`SYSTEMS_RETE` is the same data as a graph, together with the house. The engine loads it for SPARQL and falls back to the
house-only file if it is not there; `app.graphSource` says which one it got.

## The services layer (a designed example, not part of the IFC)

Everything is drawn from the JSON (a few hundred nodes and runs; the counts are in the data and the checks read them): nodes, runs, groups, `feeds`, `controls`, `states`, `scenarios`, `thermal`.

**Parts.** `fixtures.js` builds each node type from boxes, cylinders, spheres and tori, at its size (`size` is
`[width, depth, height]`; depth runs along `dir`). A fixture's static geometry is merged into one mesh per material, so it
costs 1 to 4 draw calls; levers (breakers, valves), lamps and LEDs are separate small meshes. A fixture also carries an
invisible box so thin plates are easy to click and aim at. Two to three meshes per node. Draw calls (1400 x 850, house
included, first version of the data): 190 without the layer, 254 with lighting (the default), 865 with every system on; `get_performance` reports today's. Nothing renders unless something
changed.

**Placement.** `p` is a wall plate's point or a body's centre. `systems.js` ray-casts against the walls and slabs and
puts each plate with its back on the wall face, each ceiling fixture on the ceiling (tilted on the sloped roof), and
sets a body's back against the wall when it stands off by less than 30 cm. Some fixtures move more than 5 cm that way (the
bathroom's installation wall, see below); the pipes that end at them get a short stub so they still reach. Outdoor units
keep their stand-off.

**Runs.** One tube per run along its `path` at the run's diameter. Cables are drawn at least 1.5 cm thick (1.25 times their
true diameter, never under 15 mm) so they read; pipes and ducts are true size. Cold and hot water have their own colours.
Underfloor loops are drawn as a serpentine at the loop's `spacingM` inside its rectangle, just above the slab.

**What is live.** A thing is live when it is enabled and every thing that feeds it is live. Only these types switch things off:
grid connection, RCD, breaker, valve, heat pump (both units), water heater, floor loop, ventilation unit, router, network
switch, access point (state `off` or `closed`). A light, switch, hood or A/C that is `off` is idle, not dead. Not-live parts are
grey with a small red marker; lamps go dark; the info panel says why (`no power: breaker C3 is off`). A run is live when
both ends are. The scenario's `affected` list is exactly the things downstream of its `off` nodes (this is checked against the
JSON and the SPARQL answer).

**Switches.** A switch turns the lights it `controls` on or off and the other switch of a two-way pair follows. In walk mode E on a
switch does the same. Each room has one real point light, placed at the weighted centre of the fixtures that are on and
scaled by their share of the room's installed power; everything else glows by emission.

**X-ray** makes walls, slabs and the roof (and the room floors) translucent. **Isolate** fades the other systems and almost all
of the house. Selecting a thing inside a room turns x-ray on.

**Trace to source** follows `feeds` upstream (shortest path) to the system's source and highlights the path and the runs between
its steps. **Scenarios**: apply, highlight, list by room, show the SPARQL, run it on the graph and compare.

## The thermal model (`thermal.js`)

One temperature node per room (7), backward Euler, 10 minute steps, the day repeated until it closes (the result is a periodic day,
usually 5 to 50 repeats). 

```
C_i dT_i/dt = sum_j H_ij (T_j - T_i)                       rooms next to each other; open-plan boundaries: H = 0.34 * 216 * A
            + (H_out,i + H_vent,i) (T_out - T_i)           U*A of outside walls, roof, doors, windows; unheated: half
            + H_gnd,i (T_gnd - T_i)                        U*A of floors on the ground; T_gnd = 10.5 + 0.3 (T_mean - 10.5)
            + Q_int,i + Q_sol,i + Q_heat,i
H_vent = 0.34 * V * n   n = effective rate (heat recovery) if the ventilation unit is live, else the plain rate;
                        open window 180 m3/h, tilted 40, open outside door 300, open inner door 200 m3/h of exchange
Q_sol  = windows: I * A * (1 - frame) * g * (sunlit fraction of the beam)
         opaque:  U * A * (alpha * I - 0.9 * 63 W/m2 * cos(tilt)) / 25      (sol-air, with sky radiation)
Q_heat = Q_loops * clamp((T_set - T) / 1.5 K, 0, 1)         only if heat pump, thermostat and loop are live
```

Sun: Kasten-Young air mass, Meinel direct normal `1353 * (1 + 0.033 cos(...)) * 0.7^(AM^0.678)` (893 W/m2 at solar noon on
21 June, 627 on 21 December), diffuse from the ASHRAE monthly ratio, ground reflectance 0.2, isotropic sky. Clearness `k`: beam
`* k^2`, diffuse `k * clear + (1 - k) * 0.2 * clear global`. Shading: 9 rays per window from the glass plane toward the sun
against the walls and slabs (eaves, reveals, the building itself); cached per date, site and north. Outdoor
temperature is a cosine between the minimum (03:00) and the maximum (15:00), typical values per month for Karlsruhe typed
in by hand: a typical-day approximation, not weather data. Optional blinds close above 200 W/m2 on a window (off by default).

It is fine for comparing orientations and what-ifs, not a certified calculation. Rooms are single nodes, the sky is clear,
insulation and g-values are assumed (see `thermal.assumptions`), no shading by people.

What a real house does in summer is in the model, as three options in the Thermal panel (on by default, switch them off for
the plain model):
- **summer bypass**: the ventilation unit stops recovering heat (the full air change rate instead of the effective one)
  while the room is above its set point, the outdoor air is cooler than the room and at least 10 C (in winter nothing changes);
- **night ventilation**: 2 extra air changes per hour while the sun is down, the room is above 23 C, the outdoor air is
  more than 1 K cooler and at least 10 C;
- **external blinds**: a sunlit window lets 20 % of its gain through while its room is above 24 C (fully closed from 25 C).
The rule uses the room temperature at the end of the previous 10 minute step. With windows shut on a clear 21 June the rooms
peak at 24 to 28 C (about 26 C); with the three options off the old result comes back (32 to 39 C: nothing removes the heat).
December is the same with them on or off. `run_thermal_day` takes `summerBypass`, `nightVentilation` and `blinds` (default true).
The trees of the scenery shade the windows only with "Trees shade the house" on (default off).

## Draw calls: batches, proxies, offsets

The house and the services are made of hundreds of small elements. Each keeps an **invisible proxy** mesh in the scene (material
`app.PROXY`: not drawn, still ray cast): picking, bounding boxes, the walk collision, the thermal shading and the placement
of fixtures all use the proxies. What is *drawn* comes from `batch.js`: parts that share a texture, side, shadow flags,
environment and polygon offset are one merged geometry (a "batch"), two meshes per batch (opaque and see-through). Colour,
roughness, metalness, emissive and alpha are per-vertex data (`color`, `aMat`, `aEm`); a small patch in the standard material
reads them. So:
- **highlight, dimming, x-ray, live/dead, glow** are writes into the vertex range of one element (`paint()`, `setStyle`, `setLook`);
  `app.lookOf(mesh)` reads back what is drawn (colour, alpha, emissive);
- **visibility** is the index buffer: before every render `sync()` checks the visibility chain of each element's proxy
  (class, storey, layer, roof toggles all set `.visible` on the proxy or its groups) and rebuilds the index of the batches that
  changed; hidden elements cost nothing and cast no shadow;
- **doors, windows, levers** are dynamic elements: their vertices are written again through the proxy's matrix when they move;
- **picking** ray casts the proxies (an element's `userData.alpha` follows `paint()`, so dimmed context is not picked);
  the merged meshes themselves are not ray cast;
- **"by class"** is a second set of batches, built the first time it is used.
Draw calls at 1400 x 850 (first version of the data): house 33 (before 190), default layers 37 (254), every system on 48 (865), the scenery adds up to 11.

**Z-fighting.** Faces of different elements in one plane would flicker. The element with the lower polygon offset wins:
frames -1, bars, handles and sashes -2, glass +1 (moving) or +2 (fixed), walls and members 0, beams -1, roof slabs -2, floor
slabs +3, the lawn plate +4, room floor decals -3 (and 1.5 cm short of each wall); services: material groups of one fixture
0 to 3, runs 4 to 6 by system, unknown node types 7; fixtures on a wall stand 1 mm off it. `await app.findCoplanar()` lists
the pairs of visible faces that share a plane (within 1.2 mm, facing the same way or double-sided, overlapping by more than 5 cm2)
and says which have a rule; `check_building.mjs` fails on any without one. The camera planes are tight: the near plane follows
the distance to the house (0.1 to 2 m, 0.08 m in walk mode), the far plane is 1100 m (450 m in walk mode), and the sky is drawn at
infinity (`gl_Position.xyww`) so it does not set it. A logarithmic depth buffer was not needed.

## Quality, shadows, lights

Three levels, chosen from the device (small screen, few cores or little memory) and switchable (`set_quality`, the Look panel,
`?quality=low|medium|high`): **low** pixel ratio 1, no shadows, little scenery, still; **medium** 1.5, 1024 px shadows, scenery at
24 frames/s; **high** 2, 2048 px, full scenery. A device that keeps missing 20 fps steps down one level (not under WebDriver).
The shadow map is redrawn only when the sun moves, a door or window moves or something in the house is shown or hidden
(`autoUpdate` is off) and its camera is fitted to the house each time. Lights are never added or removed (intensity 0 instead),
so the shader programs stay the same through day, night, lights and layers.

## Scenery

`scenery.js`: a dithered sky dome (Bayer 8x8 anchored to screen pixels; strength is a setting), the sun disc (about 2.5 degrees
across, at the computed position) and the moon, stars, clouds; a ground disc with dithered grass tones, a stone path to the front
door; trees, bushes, rocks, tufts, flowers and a fence (InstancedMesh, fixed seed, the house, the path and the view from the
doors stay clear); birds and fireflies. Sway, clouds, birds and fireflies run in the shaders. Cost: 11 draw calls, 4 materials.
The animation asks for frames at the level's rate, stops when the tab is hidden or the canvas is off screen, and does not run with
`prefers-reduced-motion` or wind 0. Trees and the scenery are not pickable and not solid (the walker stays inside the plot, `app.plot`);
they are hidden in x-ray and isolate. `set_scenery` / the Scenery control: on, dither, wind, trees shade the house.

## How doors and windows move

Each door/window mesh is split into connected components. A window keeps its outer frame (and mullion) and each casement
hangs on its own hinge at the outer side (`tilted` turns it about its bottom edge, 13 degrees). A casement is the pane, a sash
ring around it and a handle: the GLB has no separate sash, so the ring (4 thin boxes) and the handle are built in code; the
hinge comes from `openings[].leaves` where the data has it (`hingeSource: 'data'`), else from the geometry. A door keeps a frame
if it has one; the leaf with its handle swings about the hinge of the data (`openings[].hinge`, `closed`, `opens`; all five doors have
it) or, without the data, about the end opposite its handle. **Sliding** doors (`operation: "sliding"`, `slide`, `travel`; without the
data: two or more wide thin panels in a door wider than 1.5 m): the panel at the start end of the slide moves along `slide` by `travel`
on a track 3 mm off the others, no rotation; the other panels, their bars and the frame stay. The terrace door has three panels: the
first slides 0.67 m in front of the second, the other two are fixed; the open door has a clear width of about 0.6 m.

## Collision

2D obstacles per storey: walls (cut at door openings), doors (while not open; for a sliding door every panel, the moving one where it stands), the edge of the plot, railings, the stair (a circle). Windows
stay part of the wall. Circle of 0.25 m, 4 cm steps, slide along walls. Upper floor: a ray down must hit the slab and a
ray up must leave 1.75 m under the roof. The stair is a trigger: E nearby plays a 3 s climb.
