import { Button, createTheme, rem } from "@mantine/core";
import { FONT_FAMILY, leaf, piep } from "@shared/design/tokens";

export const theme = createTheme({
  primaryColor: "piep",
  primaryShade: { light: 6, dark: 5 },
  defaultRadius: "md",
  colors: { piep, leaf },
  fontFamily: FONT_FAMILY,
  fontFamilyMonospace: '"Cascadia Code", "SFMono-Regular", Consolas, monospace',
  headings: {
    fontFamily: FONT_FAMILY,
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
    Button: Button.extend({
      defaultProps: { radius: "md" },
      // 地の薄いボタンは、色を指定しなければ灰色にする。app.css が Mantine の
      // ホバー色を常時の地に使うので、既定の青のままだと濃い文字に強い青の地が
      // 付き、light より目立って主操作と競う。赤などは color で渡せば残る。
      vars: (_theme, props) => ({
        root: props.variant === "subtle" && !props.color
          ? {
              "--button-color": "var(--mantine-color-gray-light-color)",
              "--button-hover": "var(--mantine-color-gray-light-hover)",
              "--button-hover-color": "var(--mantine-color-gray-light-color)",
            }
          : {},
      }),
    }),
    ActionIcon: { defaultProps: { radius: "md" } },
    Card: { defaultProps: { radius: "lg", withBorder: true } },
    Accordion: { defaultProps: { radius: "lg" } },
    Paper: { defaultProps: { radius: "lg" } },
    Modal: { defaultProps: { radius: "lg", centered: true, padding: 24, closeButtonProps: { size: 32, "aria-label": "閉じる" } }, styles: { header: { padding: 24, minHeight: 80, gap: 16, flexShrink: 0 }, title: { minWidth: 0, overflowWrap: "anywhere" }, close: { flexShrink: 0 } } },
    // Keep the size when a screen overrides only closeButtonProps' accessible label.
    ModalCloseButton: { defaultProps: { size: 32 } },
    Drawer: { defaultProps: { overlayProps: { backgroundOpacity: 0.42, blur: 4 }, closeButtonProps: { "aria-label": "閉じる" } } },
    Tooltip: { defaultProps: { openDelay: 420, withArrow: true } },
  },
});
