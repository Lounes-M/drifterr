# Satoshi font — drop the files here

Upload the Satoshi font files into **this folder** (`apps/desktop/ui/public/fonts/`).
The CSS (`apps/desktop/ui/styles.css`) is already wired to them via `@font-face`,
and the proxy serves this folder at `/public/fonts/*`, so once the files are
here they load in both the browser dashboard and the Tauri menubar — no other
changes needed.

## What the CSS uses (required)

The stylesheet references the **variable** files, which cover every weight:

- `Satoshi-Variable.woff2`  ← primary
- `Satoshi-Variable.woff`   ← fallback
- `Satoshi-VariableItalic.woff2`
- `Satoshi-VariableItalic.woff`

## Also fine to upload (the full family, harmless if unused)

```
Satoshi-Light.woff(2)        Satoshi-LightItalic.woff(2)
Satoshi-Regular.woff(2)      Satoshi-Italic.woff(2)
Satoshi-Medium.woff(2)       Satoshi-MediumItalic.woff(2)
Satoshi-Bold.woff(2)         Satoshi-BoldItalic.woff(2)
Satoshi-Black.woff(2)        Satoshi-BlackItalic.woff(2)
```

Until the files are present, the UI falls back to the system sans-serif — nothing
breaks, it just won't be Satoshi yet.
