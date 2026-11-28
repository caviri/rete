# Viewport API (layout, sections, camera modes, structure, state variables, reset)

For the dashboard: everything below is driven through `window.reteBuilding` (the `app` object). The viewport pass builds no UI chrome except what
belongs inside a pane (label, border, a maximise button, a cut scrubber). All frames are model metres, Z up, the same as `README.md`.
Each module has the same text in the comment at the top of its file. The WebMCP tools are `viewportTools(app)` in `webmcp-viewport.js`
(14 tools, same shape as in `webmcp.js`): register them with `addTools(viewportTools(app))`.

Everything is optional-safe: with the systems file missing `app.structure`, `app.statevars` and `app.svc` are `undefined`; the viewport, sections,
fly mode, ground cutaway and reset exist anyway.

## `app.viewport` (viewport.js)

One WebGL canvas, one renderer. Panes are drawn with viewport + scissor. Events: `layout` `{layout, maximised, panes}`, `pane` `{id, change}`,
`hover` `{pane, id, label}`.

| Call | |
| --- | --- |
| `setLayout('single' \| 'quad')`, `getLayout()` | quad = plan (top left), 3D (top right), front elevation, side elevation. Entering quad switches the z cut on for the plan pane |
| `maximise(paneId \| null)`, `getMaximised()` | `plan` `front` `side` `3d`; one pane fills the viewport. Walk and fly in a quad maximise the 3D pane by themselves and give it back |
| `panes()`, `pane(id)` | descriptors: `id, label, projection, visible, rect {x,y,w,h}, maximised`; orthographic: `view {centre, pixelsPerMetre, scale}`, `axes`, `facade {name:'south-east', azimuthDeg, normal}`, `cut {axis, enabled, position}`, `lookDirection`; 3D: `view {fovDeg, position, mode}` |
| `setView(id, {centre:[a,b], pixelsPerMetre})`, `zoomBy(id, f)`, `fit(id?)` | pan and zoom an orthographic pane. Plan axes are (x, y), front (x, z), side (y, z) |
| `setNorthUp(bool)` | plan turned so that true north is up (default: model axes, a compass arrow shows north) |
| `pick(clientX, clientY, paneId?)` | `{pane, id, pick, dist, point}`: what is under the pointer, ignoring what a cut removed. `paneAt(x, y)` |
| `project(paneId, [x,y,z])` | world point to `{x, y}` in CSS px of the viewport |
| `invalidate(paneId?)`, `stats()` | redraw; `{frames, drawn:{pane: n}, drawCalls}` |
| `setBackground('#hex')` | background of the orthographic panes |

Façades: the front elevation looks at the face whose outward normal is -y, the side elevation at +x. Their names come from the site's true north
(here "south-east" and "north-east"). Orthographic panes: wheel or pinch zooms about the pointer, drag pans, click selects (a selection made elsewhere
pans the pane to it, without changing its zoom unless it does not fit), double click opens or closes a door or window. They draw no sky, scenery,
shadows or time-of-day light. Phone width: the quad stays a 2x2 grid; every pane has a maximise button.

Picking by the page: ui.js picks in the 3D pane with the canvas rectangle, which now answers with the 3D pane's rectangle. In the ortho panes the viewport
picks itself and calls `app.select(id)`. If ui.js is rewritten, call `app.viewport.pick(clientX, clientY)` for a click anywhere.

## `app.section` (section.js), event `section`

`set(axis, {enabled, position, flip})` (axis `x|y|z`), `get()`, `reset()`, `setApplyTo3D(true|false|null)`, `setStorey(i)`, `storeys()`, `setHandles(bool)`.
Kept side: z: `z <= position`; y: `y >= position`; x: `x <= position`; `flip` keeps the other. Plan obeys z, front y, side x; the 3D pane all enabled cuts
when `applyTo3D` (default: on in single, off in quad; a draggable translucent plane in the 3D pane). `setStorey(i)`: z cut 1.2 m above that floor.
Cut faces: the back faces of the closed solids are drawn as a darker hatched flat colour of their material (no stencil); pipes and embedded structure show.

## `app.camera` modes (the THREE camera, extended)

`camera.setMode('orbit'|'walk'|'fly', {position:[x,y,z], look:[dx,dy,dz]})` returns a Promise of `{mode, position, yaw, pitch}`; `camera.getMode()`;
`camera.pose()`. Event `mode` `{mode}` (also from the old `app.walk`, which keeps working). Fly (`app.fly`, loaded on first use with `app.loadFly()`):
`enter`, `leave`, `info`, `setSpeed`, `move`, `look`; WASD/arrows, Space/E up, Ctrl/Q down, Shift fast, wheel speed, mouse look (pointer lock, drag as a fallback),
touch stick plus a vertical slider. Esc leaves.

## `app.ground` (ground.js), event `cutaway`

`app.ground.setCutaway('none'|'trench'|'hidden', {structure:true})`, `getCutaway()`, `trench()`. Trench: the ground is cut away around the footprint down to the
foundation base (-1.1 m), earth-coloured lining. It also switches the Structure layer on.

## `app.structure` (structure.js), events `structure`, `structurepath`, `structurecolour`

`list({role, loadBearing, fromIfc, embedded})`, `setColourMode('material'|'role'|'load-bearing'|'from-ifc')`, `getColourMode()`, `legend()`,
`tracePath(idOrGuid, 'down'|'up')`, `clearPath()`, `remove(idOrGuid)`, `restore(idOrGuid?)`, `state()` `{removed, unsupported:[{id,label,reason}]}`,
`stateOf(id)`, `info(id)`, `applyScenario(id)`, `clearScenario()`. The rule is computed in the page: an element is supported when one complete
`bearsOn` path to `ground` exists through intact elements. Elements with `embedded: true` are drawn only in x-ray, isolate, with a cut or a load path.

### Scenarios of both kinds (the panel needs no special case)

`app.svc.scenarios` lists all 15; each has `kind: 'supply' | 'structure'`. `app.svc.applyScenario(id)` handles both and returns the same shape
`{id, label, description, off, affected, affectedRuns, expected, same, sparql}` (plus `kind`, `reasons` for structure); it emits `scenario`.
`app.svc.resetStates()` clears either kind and restores removed elements. `app.svc.runScenarioQuery(id)` for a structure scenario returns
`{approximate: true, same: <the exact set is contained in the graph's answer>}`: the SPARQL of a structure scenario is a superset, not the answer.

## `app.statevars` (statevars.js), events `value`, `values`, `colour`

`variables({kind}|{thing})`, `variable(id)`, `thingKind(id)`, `appliesTo(thingId)`, `things(kind)`, `getValue(var, thing)`, `setValue(var, thing, value)`
(throws on range, enum, unknown thing; a number written for a variable the simulation owns pins it), `unpin`, `pinned()`, `levelOf(var, thing, value?)`,
`allowedStates(thingId)`, `colourBy(var|null)` returns `legend()` = `{variable, label, unit, unitLabel, relativeTo, items:[{id,label,color,min,max,text}], things:[{id,label,value,level,color}]}`,
`rules()`, `supplyBlocking()`, `resetValues()`. The thermal model writes room temperature, air change rate, and per exterior surface sunlit fraction and
irradiance into the store (`app.thermal.publish()`), reads set points and pinned temperatures from it. `thermal-ui.js` can replace its own ramp with
`app.statevars.colourBy('room-air-temperature')` and `legend()`.

## `app.reset(scope)` (reset.js), event `reset`

`'view' | 'states' | 'time' | 'simulation' | 'all'`, returns `{scope, changed: [...]}`.

## Rendering schedule

`app.requestRender()` redraws every pane; `app.requestRender('3d')` only the 3D pane; `addTicker(fn, {scope: '3d'})` for a ticker that only moves the 3D camera.

## `app.rooms` (rooms.js), event `roomstyle`

How a room is drawn. `setStyle('outline' | 'floor' | 'fill' | 'off')` (default `'outline'`), `getStyle()`, `outline(guid)`, `floor(guid)`, `insideRoom()`.
Room volumes are never drawn filled by default: a shown room (class "Rooms" on) is a faint border, a highlighted one (selected, adjacent, a query result) a bright
border in the highlight colour. `floor` adds a tint of the floor of a highlighted room. `fill` is the old translucent volume and is never drawn for the room the
camera is inside (walk and fly). Colour-by-variable on rooms tints the floor and draws the border; its volume only shows in the `fill` style. Add it to the View menu.
`reset('view')` puts the style back to `outline`.

## Windows (states.js)

A two-leaf window here is a Stulpfenster: the IFC mesh is one solid (outer frame and centre bar together; the openings data has two leaves and no mullion), so the
triangles of the frame that are not on its outer ring (the meeting stile) are cut out and each half moves with its leaf; only the outer frame stays fixed.
Tilted: only the first leaf (the one with the handle) tilts; the other stays shut. A fixed window has the one state "closed" (from `state-window-fixed`): `setState(id, "open")` throws, and the inspector and the States list show no button for it.

## Frame cost

Measured under SwiftShader on the desktop orbit: no attribute upload, no index rebuild, no shadow-map redraw and 0.1 ms of `sync()` per orbit frame; the time is
raster: scenery about 45 ms, the ground and contact plane about 25 ms, and half the pixels saves about a third. Quad adds one orthographic pass per pane, each
without shadows, sky or scenery, drawn only when its own content changed.
