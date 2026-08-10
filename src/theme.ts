import { createTheme, rem } from "@mantine/core";

const piep = [
  "#edf8ff",
  "#d8efff",
  "#b8e2ff",
  "#8dd1ff",
  "#59b8ff",
  "#279cff",
  "#0d86f4",
  "#0872d3",
  "#0b61ad",
  "#0c518d",
] as const;

const leaf = [
  "#f4fae8",
  "#e7f3cf",
  "#d0e9a3",
  "#b5dc72",
  "#9dcf49",
  "#86b918",
  "#729f12",
  "#5e8310",
  "#506b13",
  "#455b15",
] as const;

export const theme = createTheme({
  primaryColor: "piep",
  primaryShade: { light: 6, dark: 5 },
  defaultRadius: "md",
  colors: { piep, leaf },
  fontFamily:
    '"Segoe UI", "Yu Gothic UI", Meiryo, "Hiragino Sans", system-ui, -apple-system, sans-serif',
  fontFamilyMonospace: '"Cascadia Code", "SFMono-Regular", Consolas, monospace',
  headings: {
    fontFamily:
      '"Segoe UI", "Yu Gothic UI", Meiryo, "Hiragino Sans", system-ui, -apple-system, sans-serif',
    fontWeight: "680",
    sizes: {
      h1: { fontSize: rem(30), lineHeight: "1.16" },
      h2: { fontSize: rem(23), lineHeight: "1.22" },
      h3: { fontSize: rem(18), lineHeight: "1.3" },
    },
  },
  cursorType: "pointer",
  focusRing: "auto",
  components: {
    Button: { defaultProps: { radius: "md" } },
    ActionIcon: { defaultProps: { radius: "md" } },
    Card: { defaultProps: { radius: "lg", withBorder: true } },
    Paper: { defaultProps: { radius: "lg" } },
    Modal: { defaultProps: { radius: "lg", centered: true } },
    Drawer: { defaultProps: { overlayProps: { backgroundOpacity: 0.42, blur: 4 } } },
    Tooltip: { defaultProps: { openDelay: 420, withArrow: true } },
  },
});
