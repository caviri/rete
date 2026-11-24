// Tailwind v3 config used only to GENERATE css: shim.css (standalone viewer), content/retelens.css
// (the classes Hugging Face's own stylesheet lacks) and content/github.css (all classes, with the
// colour palette mapped to GitHub's Primer CSS variables so it follows the user's GitHub theme).
const target = process.env.RETELENS_TARGET || "";
const safelist = process.env.RETELENS_SAFELIST ? require(process.env.RETELENS_SAFELIST) : [];
const primer = {
  white: "var(--bgColor-default, #fff)",
  black: "var(--fgColor-default, #1f2328)",
  gray: {
    50: "var(--bgColor-muted, #f6f8fa)", 100: "var(--borderColor-muted, #d1d9e0b3)", 200: "var(--borderColor-default, #d1d9e0)",
    300: "var(--borderColor-default, #d1d9e0)", 400: "var(--fgColor-muted, #59636e)", 500: "var(--fgColor-muted, #59636e)",
    600: "var(--fgColor-muted, #59636e)", 700: "var(--fgColor-default, #1f2328)", 800: "var(--fgColor-default, #1f2328)",
    900: "var(--fgColor-default, #1f2328)", 850: "var(--bgColor-muted, #f6f8fa)", 925: "var(--bgColor-muted, #f6f8fa)",
    950: "var(--bgColor-default, #fff)",
  },
  blue: { 400: "var(--fgColor-accent, #0969da)", 600: "var(--fgColor-accent, #0969da)" },
};
// Zenodo (InvenioRDM on Semantic UI): light only, Helvetica, #2f6fa7 links, #f5f5f5 / #e3e3e3 sidebar greys.
const zenodo = {
  white: "#fff", black: "#000",
  gray: { 50: "#f9f9f9", 100: "#ededed", 200: "#e3e3e3", 300: "#d4d4d5", 400: "#8a8a8a", 500: "#696969", 600: "#555", 700: "#3b3b3b", 800: "#1b1c1d", 850: "#1b1c1d", 900: "#1b1c1d", 925: "#1b1c1d", 950: "#1b1c1d" },
  blue: { 400: "#2f6fa7", 600: "#2f6fa7", 700: "#246298" },
};
module.exports = {
  content: process.env.RETELENS_SAFELIST ? [] : target === "github"
    ? ["./lib/*.js", "./content/adapters/github.js"]
    : target === "zenodo" ? ["./lib/*.js", "./content/adapters/zenodo.js"]
    : ["./lib/*.js", "./viewer.js", "./content/adapters/hf.js"],
  safelist,
  darkMode: "class",
  theme: {
    extend: {
      colors: target === "github" ? primer : target === "zenodo" ? zenodo : { gray: { 850: "#141c2e", 925: "#101623", 950: "#0b0f19" } },
      fontFamily: target === "zenodo"
        ? { sans: ['"Helvetica Neue"', "Helvetica", "Arial", "sans-serif"], mono: ["Menlo", "Consolas", "Monaco", "monospace"] }
        : target === "github"
        ? { sans: ["var(--fontStack-system, -apple-system, 'Segoe UI', sans-serif)"], mono: ["var(--fontStack-monospace, ui-monospace, monospace)"] }
        : {
            sans: ['"Source Sans 3"', "Source Sans Pro", "-apple-system", "BlinkMacSystemFont", '"Segoe UI"', "Roboto", "sans-serif"],
            mono: ['"IBM Plex Mono"', "ui-monospace", "SFMono-Regular", "Menlo", "Monaco", "Consolas", "monospace"],
          },
    },
  },
  plugins: [],
  corePlugins: { preflight: !process.env.RETELENS_SAFELIST && target !== "github" && target !== "zenodo" },
};
