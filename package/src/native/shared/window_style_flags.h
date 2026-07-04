// window_style_flags.h - Bit layout for the packed getWindowStyle input.
//
// getWindowStyle takes a single u32 whose low 12 bits are the requested window
// style flags. This replaces the previous 12-separate-bool signature: Bun's
// arm64 FFI silently drops bool arguments past the register slots (positions
// 9-12), which disabled NonactivatingPanel/DocModalWindow/HUDWindow. One u32
// is register-passed and marshals reliably; the TS side packs, native unpacks.
// The bit order matches the historical argument order so the mapping is a
// mechanical translation.

#ifndef ELECTROBUN_WINDOW_STYLE_FLAGS_H
#define ELECTROBUN_WINDOW_STYLE_FLAGS_H

enum ElectrobunWindowStyleFlag {
    EB_STYLE_BORDERLESS = 1u << 0,
    EB_STYLE_TITLED = 1u << 1,
    EB_STYLE_CLOSABLE = 1u << 2,
    EB_STYLE_MINIATURIZABLE = 1u << 3,
    EB_STYLE_RESIZABLE = 1u << 4,
    EB_STYLE_UNIFIED_TITLE_AND_TOOLBAR = 1u << 5,
    EB_STYLE_FULL_SCREEN = 1u << 6,
    EB_STYLE_FULL_SIZE_CONTENT_VIEW = 1u << 7,
    EB_STYLE_UTILITY_WINDOW = 1u << 8,
    EB_STYLE_DOC_MODAL_WINDOW = 1u << 9,
    EB_STYLE_NONACTIVATING_PANEL = 1u << 10,
    EB_STYLE_HUD_WINDOW = 1u << 11,
};

#endif // ELECTROBUN_WINDOW_STYLE_FLAGS_H
