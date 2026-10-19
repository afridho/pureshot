---
name: PureShot Theming and Contrast
description: Use when adding or changing light/dark theme behavior, colors, backgrounds, borders, controls, or text contrast in PureShot's React/WebView settings UI. Ensures Windows theme following and prevents unreadable inherited colors.
---

# PureShot Theming and Contrast

Load this skill before changing colors, themes, or appearance in the PureShot React/WebView UI.

## Scope

The settings UI is implemented in:

- `src/App.tsx`
- `src/styles.css`

PureShot should follow the Windows app theme automatically using:

```css
:root { color-scheme: light dark; }

@media (prefers-color-scheme: dark) {
  /* dark overrides */
}
```

Do not add a separate persisted theme preference unless the user explicitly asks for one.

## Critical cascade warning

`src/styles.css` contains broad light-theme rules, including a global declaration similar to:

```css
html, body, #root { color: #17191d; }
```

A dark-mode rule placed before that declaration can lose because both rules have equal specificity and the later declaration wins. This caused near-black setting labels on dark cards.

When adding dark mode:

1. Prefer semantic CSS variables declared on `:root` and overridden inside `prefers-color-scheme`.
2. If keeping direct overrides, make the dark selector more specific than later global rules.
3. Set foreground colors on the actual themed containers, such as `.settings-workspace`, `.settings-content`, and `.settings-sidebar`, not only on `html/body/#root`.
4. Explicitly cover important text roles rather than trusting inheritance through broad legacy rules.

## Recommended semantic palette

Use variables for new theme work:

```css
:root {
  color-scheme: light dark;
  --surface-page: #ffffff;
  --surface-sidebar: #fafafa;
  --surface-card: #f6f6f7;
  --text-primary: #17191d;
  --text-secondary: #858a92;
  --border-subtle: #e6e7e9;
}

@media (prefers-color-scheme: dark) {
  :root {
    --surface-page: #202020;
    --surface-sidebar: #191919;
    --surface-card: #2b2b2b;
    --text-primary: #f1f3f5;
    --text-secondary: #aeb3bb;
    --border-subtle: #3b3b3b;
  }
}
```

New rules should consume these variables instead of introducing more isolated hex colors.

## Required text coverage

Check every one of these in both light and dark modes:

- Page title (`h1`)
- Section titles (`.settings-group > h2`)
- Setting labels (`.setting-row strong`)
- Setting descriptions (`.setting-row small`)
- Sidebar profile title and subtitle
- Sidebar normal, hover, and selected labels
- About page title, body, and version
- Dropdown trigger, menu options, selected option, and checkmark
- Keyboard shortcut labels
- Range value labels
- Buttons and their icons
- Native window control icons

Do not assume `color` inherited from `body` is enough.

## Contrast targets

Use WCAG contrast as the minimum quality bar:

- Normal text: at least **4.5:1** against its immediate background.
- Large or bold text: at least **3:1**.
- Controls, borders, icons, and focus indicators: at least **3:1** against adjacent colors where needed to identify the component.
- Secondary text should remain visibly secondary but must not become faint or gray-on-gray.

Useful dark defaults:

- Primary text on `#202020` or `#2b2b2b`: `#f1f3f5`
- Secondary text on those surfaces: `#aeb3bb` or lighter
- Subtle borders on cards: around `#3b3b3b`

Avoid primary text darker than its card and avoid low-opacity text unless contrast has been checked.

## Control states

For each interactive control, inspect:

1. Default
2. Hover
3. Active/pressed
4. Selected/checked
5. Focus-visible
6. Disabled, if present
7. Dropdown open state

A state must not communicate only through a tiny color difference. Keep labels and icons readable in every state.

## Verification workflow

After editing theme CSS:

1. Inspect the cascade order in the full `src/styles.css`, especially global selectors that occur after dark overrides.
2. Run `git diff --check`.
3. Open the app in both Windows light and dark app modes.
4. Visit every settings section: General, Shortcuts, Quick Access, Screenshots, Advanced, and About.
5. Open every custom dropdown and test hover/selected states.
6. Check the custom title-bar controls.
7. If browser tooling is available, inspect computed `color` and `background-color` for any suspicious text.
8. Capture screenshots in both themes before declaring the work complete.

For CSS-only changes, Tauri hot reload should usually be sufficient. Native restart instructions are not needed unless Rust code also changed.

## Completion checklist

- [ ] Follows Windows light/dark app theme automatically
- [ ] Primary labels are readable on cards
- [ ] Secondary descriptions retain adequate contrast
- [ ] Sidebar states are readable
- [ ] Dropdown menu and trigger are readable
- [ ] Controls and icons are distinguishable
- [ ] No later global rule overrides dark foreground colors
- [ ] All settings sections checked in both themes
- [ ] `git diff --check` passes
