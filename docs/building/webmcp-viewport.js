// WebMCP tools of the viewport pass: layout and panes, section cuts, camera modes, ground cutaway, structure, state variables, reset.
// `viewportTools(app)` returns the array; the page registers it (addTools from webmcp.js) like the others. Same conventions: JSON results
// of at most about 18 KB, errors are thrown with a message that says what to do.
const MCP_MAX = 18000;
const ro = { readOnlyHint: true };
const obj = (properties, required) => ({ type: 'object', properties: properties || {}, required: required || [], additionalProperties: false });
const capList = (o, key) => { while (JSON.stringify(o).length > MCP_MAX && o[key] && o[key].length > 1) { o[key] = o[key].slice(0, Math.floor(o[key].length * 0.8)); o.truncated = true; } return o; };

export const viewportTools = app => {
  const vp = () => { if (!app.viewport) throw new Error('The viewport is not ready yet.'); return app.viewport; };
  const sv = () => { if (!app.statevars) throw new Error('The state variables are not loaded (the systems file is missing).'); return app.statevars; };
  const st = () => { if (!app.structure) throw new Error('The structure layer is not loaded (the systems file is missing).'); return app.structure; };
  const sec = () => { if (!app.section) throw new Error('The section cuts are not loaded.'); return app.section; };
  const gnd = () => { if (!app.ground || typeof app.ground.setCutaway !== 'function') throw new Error('The ground cutaway is not loaded.'); return app.ground; };
  const view = () => ({ layout: vp().getLayout(), maximised: vp().getMaximised(), panes: vp().panes(), section: app.section ? app.section.get() : null, cameraMode: app.mode, cutaway: app.ground && app.ground.getCutaway ? app.ground.getCutaway() : 'none' });
  return [
    { name: 'set_layout', description: 'Set the 3D view layout: "single" (the 3D view) or "quad" (plan from above, 3D, front elevation and side elevation, the elevations to scale). Optionally maximise one pane of the quad (plan, front, side, 3d) or "none".',
      inputSchema: obj({ layout: { type: 'string', enum: ['single', 'quad'] }, maximise: { type: 'string', description: 'plan, front, side, 3d, or none to show all four.' } }, ['layout']),
      execute: async a => { vp().setLayout(a.layout); if (a.maximise != null) vp().maximise(a.maximise === 'none' ? null : a.maximise); return view(); } },
    { name: 'get_viewport', annotations: ro, description: 'The layout, every pane with its projection, size in pixels, scale, the façade an elevation shows (from true north), the cut it obeys, the section cuts, the camera mode and the ground cutaway.',
      inputSchema: obj(), execute: async () => ({ ...view(), stats: vp().stats() }) },
    { name: 'set_section', description: 'Cut the building along a model axis: the part nearer the viewer is removed and the cut faces drawn solid, so what is inside walls and slabs (lintels, ring beams, pipes) shows. z keeps what is below the position (the plan pane), y keeps y >= position (front elevation), x keeps x <= position (side elevation); flip keeps the other side. Give storey to put the z cut 1.2 m above that floor. apply_to_3d cuts the 3D view too (the default is on in the single layout, off in the quad).',
      inputSchema: obj({ axis: { type: 'string', enum: ['x', 'y', 'z'] }, enabled: { type: 'boolean' }, position: { type: 'number', description: 'Metres, model frame.' }, flip: { type: 'boolean' }, storey: { type: 'integer', description: 'With axis z: the storey whose floor the cut stands above.' }, apply_to_3d: { type: 'boolean' } }, ['axis']),
      execute: async a => {
        const S = sec(); if (a.apply_to_3d != null) S.setApplyTo3D(a.apply_to_3d);
        if (a.storey != null) S.setStorey(a.storey);
        S.set(a.axis, { enabled: a.enabled ?? (a.position != null || a.storey != null ? true : undefined), position: a.position, flip: a.flip }); return S.get();
      } },
    { name: 'set_camera_mode', description: 'Switch the camera: orbit (around the house), walk (first person, with collision) or fly (free flight through walls and below the ground, to look at the foundations). Optionally a position [x, y, z] in metres and a look direction [dx, dy, dz]. Orbit to fly keeps where the camera is; fly to walk puts the walker on the floor below.',
      inputSchema: obj({ mode: { type: 'string', enum: ['orbit', 'walk', 'fly'] }, position: { type: 'array', items: { type: 'number' } }, look: { type: 'array', items: { type: 'number' } } }, ['mode']),
      execute: async a => { if (typeof app.camera.setMode !== 'function') throw new Error('The camera modes are not loaded.'); return app.camera.setMode(a.mode, { position: a.position, look: a.look }); } },
    { name: 'set_ground_cutaway', description: 'Show what is under the lawn: "trench" cuts the terrain away around the house down to the foundation base (an excavated box with earth-coloured sides), "hidden" removes the ground and the plants, "none" puts them back. Switches the Structure layer on.',
      inputSchema: obj({ mode: { type: 'string', enum: ['none', 'trench', 'hidden'] } }, ['mode']),
      execute: async a => { const g = gnd(); g.setCutaway(a.mode); return { cutaway: g.getCutaway(), trench: g.trench() }; } },
    { name: 'list_structure', annotations: ro, description: 'The structural elements (the IFC walls, slabs, rafters and purlins, and the designed foundations, slab layers, ring beams, lintels, posts, ties and bracing) with role, whether they are load-bearing, from the IFC or designed, embedded in a wall or slab, what they bear on, and their state.',
      inputSchema: obj({ role: { type: 'string' }, load_bearing: { type: 'boolean' }, from_ifc: { type: 'boolean' }, embedded: { type: 'boolean' }, limit: { type: 'integer' } }),
      execute: async a => { const all = st().list({ role: a.role, loadBearing: a.load_bearing, fromIfc: a.from_ifc, embedded: a.embedded }); const lim = Math.min(200, Math.max(1, a.limit || 60)); const o = { total: all.length, roles: st().roles(), items: all.slice(0, lim) }; return capList(o, 'items'); } },
    { name: 'trace_load_path', description: 'Highlight what an element bears on, down to the ground ("down"), or everything that bears on it ("up"). Uses the bearsOn links; the elements embedded in walls are shown.',
      inputSchema: obj({ id: { type: 'string', description: 'An element id or IFC GUID from list_structure.' }, direction: { type: 'string', enum: ['down', 'up'] } }, ['id']),
      execute: async a => capList({ ...st().tracePath(a.id, a.direction || 'down') }, 'ids') },
    { name: 'remove_structural_element', description: 'Take a structural element out (a wall, a purlin, a lintel). Everything with no complete load path to the ground through the remaining elements is marked unsupported, with the reason.',
      inputSchema: obj({ id: { type: 'string' } }, ['id']), execute: async a => { st().remove(a.id); return capList({ removedElement: a.id, ...st().state() }, 'unsupported'); } },
    { name: 'restore_structure', description: 'Put one removed structural element back, or all of them when no id is given.',
      inputSchema: obj({ id: { type: 'string' } }), execute: async a => st().restore(a.id) },
    { name: 'list_state_variables', annotations: ro, description: 'The state variables of the data: numeric ones with range, unit, step and levels (with colours, some relative to the room set point) and enumerated ones with their states. Filter by what they apply to: room, surface, device, opening, structure, or the id of one thing.',
      inputSchema: obj({ applies_to: { type: 'string', description: 'room, surface, device, opening, structure, or a thing id.' } }),
      execute: async a => { const k = ['room', 'surface', 'device', 'opening', 'structure'].includes(a.applies_to) ? { kind: a.applies_to } : a.applies_to ? { thing: a.applies_to } : undefined; if (k && k.thing && !sv().thingKind(k.thing)) throw new Error(`No thing "${a.applies_to}".`); return capList({ variables: sv().variables(k) }, 'variables'); } },
    { name: 'get_value', annotations: ro, description: 'The current value of a state variable for one thing (a room, a surface, a device, an opening), with its level.',
      inputSchema: obj({ variable: { type: 'string' }, thing: { type: 'string' } }, ['variable', 'thing']),
      execute: async a => { const v = sv().getValue(a.variable, a.thing), lv = sv().variable(a.variable).kind === 'numeric' ? sv().levelOf(a.variable, a.thing) : null; return { variable: a.variable, thing: a.thing, value: v, level: lv ? lv.label : null, color: lv ? lv.color : null, pinned: sv().pinned().some(p => p.variable === a.variable && p.thing === a.thing) }; } },
    { name: 'set_value', description: 'Set a state variable of a thing: a room set point, a room temperature (pinned: the thermal model then holds that room at the value), occupancy, a door or window state. Throws for an unknown thing, a value outside the range or a state the thing cannot take.',
      inputSchema: obj({ variable: { type: 'string' }, thing: { type: 'string' }, value: { description: 'A number, or a state word.' } }, ['variable', 'thing', 'value']),
      execute: async a => sv().setValue(a.variable, a.thing, a.value) },
    { name: 'colour_by', description: 'Colour the rooms or the wall, roof and window faces by the level of a variable (room-air-temperature, room-air-change-rate, surface-sunlit-fraction, surface-irradiance ...). Returns the legend. Without a variable the colouring is removed.',
      inputSchema: obj({ variable: { type: 'string' } }), execute: async a => { const l = sv().colourBy(a.variable || null); return l ? capList({ ...l }, 'things') : { colouring: null }; } },
    { name: 'reset', description: 'Put the page back as it was on a fresh load: scope view (camera, layout, cuts, cutaway, colours, visibility, x-ray, quality), states (doors, windows, devices, structure, pinned values), time (date and time), simulation (scenario, traces, thermal results) or all.',
      inputSchema: obj({ scope: { type: 'string', enum: ['view', 'states', 'time', 'simulation', 'all'] } }, ['scope']), execute: async a => app.reset(a.scope) },
  ];
};
