# Fonts

Font files live in subfolders by family.

## Satoshi

Primary typeface, in [`satoshi/`](satoshi/). Already wired into the UI:

- `styles.css` references it via `@font-face`
  (`public/fonts/satoshi/Satoshi-Variable.woff2` + `.woff`, plus the variable
  italics).
- The proxy serves this tree at `/public/fonts/*`, so it loads in both the
  browser dashboard and the Tauri webview.

Source: Indian Type Foundry — https://www.fontshare.com/fonts/satoshi.
Licensed for use in this project; do not redistribute the files outside this
repository.

## Adding another family

Drop its files in a new subfolder here and add a matching `@font-face` block in
`styles.css`.
