# Brand assets

Single source of truth for the Drifterr logo / icons. The mark is the gradient
ball (orange→pink) with the white ring drifting away — the same shape the
animated CSS logo renders across the site and app.

## Masters (edit/regenerate from these)
- `app-icon-1024.png` — filled rounded-square app icon (1024², source for all app/store icons)
- `favicon-512.png` — square favicon variant (source for web favicons)
- `mark-512.png` — transparent mark only (for overlays / dark or light backgrounds)

## Where the derived sizes live
- **Web favicons** → `apps/landing/assets/favicon/` (+ `apps/landing/favicon.ico`),
  referenced from every page's `<head>`.
- **Desktop app icons** → `apps/desktop/src-tauri/icons/` (`icon.icns`, `icon.ico`,
  `32x32.png`, `128x128.png`, `128x128@2x.png`, `icon.png`, Windows `Square*Logo`,
  `StoreLogo`). The tray state icons (`tray-*.png`) are separate and not derived
  from the master.

## Regenerate
Derived icons are produced from `app-icon-1024.png` with Pillow — see the
generation step in the PR that introduced this folder, or run `tauri icon
assets/brand/app-icon-1024.png` if you have the Tauri CLI.
