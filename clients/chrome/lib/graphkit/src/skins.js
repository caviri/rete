// Class maps per host. Every key is optional; a missing key falls back to the standalone
// look (gk-* classes). "classes" given to mount() override these key by key.
// NOTE for the host: Tailwind builds purge unused classes, so safelist the classes below.
export const SKINS = {
  standalone: {},
  hf: {
    button: "inline-flex items-center justify-center gap-1 whitespace-nowrap rounded-md border border-gray-200 bg-white px-2 py-1 text-sm text-gray-700 shadow-sm hover:bg-gray-50 hover:text-gray-900 dark:border-gray-800 dark:bg-gray-900 dark:text-gray-200 dark:hover:bg-gray-850",
    buttonPrimary: "inline-flex items-center justify-center gap-1 whitespace-nowrap rounded-md border border-gray-900 bg-gray-900 px-2 py-1 text-sm text-white hover:bg-gray-700 dark:border-gray-100 dark:bg-gray-100 dark:text-gray-900",
    select: "rounded-md border border-gray-200 bg-white py-1 pl-2 pr-6 text-sm text-gray-700 shadow-sm dark:border-gray-800 dark:bg-gray-900 dark:text-gray-200",
    input: "rounded-md border border-gray-200 bg-white px-2 py-1 text-sm text-gray-800 shadow-sm placeholder:text-gray-400 dark:border-gray-800 dark:bg-gray-900 dark:text-gray-100",
    panel: "rounded-lg border border-gray-200 bg-white text-sm text-gray-800 shadow-lg dark:border-gray-800 dark:bg-gray-925 dark:text-gray-200",
    segmented: "inline-flex overflow-hidden rounded-md border border-gray-200 shadow-sm dark:border-gray-800",
    segmentedItem: "bg-white px-2 py-1 text-xs text-gray-600 hover:bg-gray-50 dark:bg-gray-900 dark:text-gray-300 dark:hover:bg-gray-850",
    segmentedItemActive: "bg-gray-100 px-2 py-1 text-xs font-semibold text-gray-900 dark:bg-gray-800 dark:text-gray-100",
    badge: "rounded-full border border-gray-200 px-1.5 text-xs text-gray-500 dark:border-gray-700 dark:text-gray-400",
  },
  github: {
    button: "btn btn-sm",
    buttonPrimary: "btn btn-sm btn-primary",
    select: "form-select select-sm",
    input: "form-control input-sm",
    panel: "Box color-shadow-large",
    segmented: "BtnGroup",
    segmentedItem: "btn btn-sm BtnGroup-item",
    segmentedItemActive: "btn btn-sm BtnGroup-item selected",
    badge: "Counter",
  },
  zenodo: {
    button: "ui small basic button",
    buttonPrimary: "ui small primary button",
    select: "ui small compact dropdown",
    input: "ui small input",
    panel: "ui segment",
    segmented: "ui small basic buttons",
    segmentedItem: "ui button",
    segmentedItemActive: "ui active button",
    badge: "ui mini basic label",
  },
};
export function resolveSkin(skin, overrides) {
  let base = {}, name = "standalone";
  if (typeof skin === "string") { name = SKINS[skin] ? skin : "standalone"; base = SKINS[name]; }
  else if (skin && typeof skin === "object") { name = skin.base && SKINS[skin.base] ? skin.base : "standalone"; base = Object.assign({}, SKINS[name], skin.classes || skin); }
  return { name, classes: Object.assign({}, base, overrides || {}) };
}
