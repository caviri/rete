import { mount, VERSION } from "./kit.js";
import { SKINS } from "./skins.js";
import { checkSkin } from "./skincheck.js";
import { PALETTE, SEQ, SHAPES } from "./util.js";

globalThis.GraphKit = { mount, version: VERSION, skins: SKINS, checkSkin, palette: PALETTE, sequential: SEQ, shapes: SHAPES };
