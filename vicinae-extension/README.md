# PureShot for Vicinae

Provides five Vicinae commands: Capture Region, Capture Fullscreen, Capture Window, Capture OCR, and Capture History.

Capture History opens PureShot's recent screenshot shelf, where captures can be browsed and restored. It requires a PureShot binary that supports the `capture-history` CLI command.

## Requirements

- Vicinae with extension support
- PureShot installed at `/home/afridho/.local/bin/pureshot` (update command entrypoints if installed elsewhere)
- OCR models installed in PureShot to use Capture Text (OCR)

## Development

From this directory run:

```sh
npm install
npm run dev
```

Keep `npm run dev` running while testing the development extension in Vicinae. To build and install it locally without a development session, run `npx vici build`; Vicinae loads the built extension from its user extension directory.
