// IFC element class -> {flat colour for the "by class" mode, pretty name, on by default}.
// Ordered for the panel.
export const CLASS_STYLE = {
  IfcWallStandardCase: { c: '#cbb89d', n: 'Walls', on: true },
  IfcSlab: { c: '#9aa0a6', n: 'Slabs / floors', on: true },
  IfcWindow: { c: '#7ec9ff', n: 'Windows', on: true },
  IfcDoor: { c: '#d98b5f', n: 'Doors', on: true },
  IfcStair: { c: '#b58cff', n: 'Stairs', on: true },
  IfcRailing: { c: '#ff9f43', n: 'Railings', on: true },
  IfcBeam: { c: '#c9a227', n: 'Beams', on: true },
  IfcMember: { c: '#8a9bb5', n: 'Members', on: true },
  IfcOpeningElement: { c: '#586074', n: 'Openings (voids)', on: false },
  IfcSpace: { c: '#4ade80', n: 'Rooms (spaces)', on: false },
};
export const CLASS_ORDER = Object.keys(CLASS_STYLE);
export const pretty = c => (CLASS_STYLE[c]?.n) || String(c).replace(/^Ifc/, '');
export const colOf = c => (CLASS_STYLE[c]?.c) || '#8895aa';
