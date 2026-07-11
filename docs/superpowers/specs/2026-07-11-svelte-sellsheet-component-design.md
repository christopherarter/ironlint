# SellSheet Component — Design Spec

## Context

This package is the core spreadsheet-style list component for **SellSheet**, a tool that lets people build a generic list of things they want to sell. The component should feel like a lightweight Notion table: keyboard-driven, drag-to-reorder, and friendly enough for non-technical users.

This repo (`sellsheet-component`) owns only the core sheet. It must be importable into a SvelteKit consumer app and previewable in a browser during development.

## Goals

- Provide a single `Sellsheet` Svelte component that renders a dynamic, editable grid.
- Support plain-text cells in the first version, with an API that can accommodate future cell types.
- Offer Notion-style interactions: inline editing, Tab/Arrow/Enter navigation, drag-to-reorder rows, and simple add-column/add-row affordances.
- Be accessible out of the box (ARIA grid roles, roving tabindex, keyboard-only use).
- Be easy to style and override from the consuming app via Tailwind utilities and CSS variables.
- Ship as a proper Svelte package that SvelteKit apps can import cleanly.

## Non-Goals

- No formula engine, sorting, filtering, or pagination in v1.
- No built-in backend/sync; state lives in the consumer.
- No non-text cell types (date, select, checkbox) in v1.

## Public API

```svelte
<script lang="ts">
  import Sellsheet from '@sellsheet/core';
  import '@sellsheet/core/styles';
  import type { Column, Row } from '@sellsheet/core';

  const columns: Column[] = [
    { key: 'item', label: 'Item' },
    { key: 'price', label: 'Price' }
  ];

  let rows: Row[] = [
    { id: 'r1', item: 'Vintage camera', price: '120' }
  ];
</script>

<Sellsheet bind:columns bind:rows />
```

### Types

```ts
export type Column = {
  key: string;
  label: string;
};

export type Row = {
  id: string;
  [key: string]: string;
};
```

### Override Props

- `class` — added to the root `role="grid"` wrapper.
- `headerClass` — added to every header cell.
- `rowClass` — added to every body row.
- `cellClass` — added to every body cell.

## Package Layout

```
sellsheet-component/
├── src/
│   ├── lib/
│   │   ├── index.ts              # public exports (Sellsheet, types, styles entry)
│   │   ├── Sellsheet.svelte      # root component
│   │   ├── components/
│   │   │   ├── HeaderRow.svelte
│   │   │   ├── BodyRow.svelte
│   │   │   └── Cell.svelte
│   │   ├── actions/
│   │   │   ├── dragReorder.ts    # pointer-based drag-to-reorder
│   │   │   └── keyboardNav.ts    # roving tabindex + edit toggling
│   │   ├── styles.css            # CSS variables
│   │   └── types.ts              # TypeScript types
│   └── routes/
│       └── +page.svelte          # live preview / playground
├── package.json
├── svelte.config.js
├── vite.config.ts
├── tsconfig.json
├── tailwind.config.js
└── README.md
```

## Component Architecture

### `Sellsheet.svelte`

- Owns state:
  - `columns` (bound)
  - `rows` (bound)
  - `activeCell: { rowIndex: number; columnKey: string } | null`
  - `editing: boolean`
- Renders:
  - `HeaderRow` with column headers and the “Add column” button.
  - `BodyRow[]` for each row.
  - “Add row” button below the grid.
- Handles:
  - Row addition/removal.
  - Column addition.
  - Drag-reorder finalization (swap rows).
  - Keyboard navigation dispatching.

### `HeaderRow.svelte`

- Renders a `role="row"` containing `role="columnheader"` cells.
- Each header is a real `<button>` for focusability and future menu actions.
- Last cell contains the “+ New column” button.

### `BodyRow.svelte`

- Renders `role="row"`.
- Includes a drag handle `<button>` on the left with an `aria-label` referencing the row index.
- Renders `Cell` for each column.

### `Cell.svelte`

- Renders as a focusable `role="gridcell"`.
- Inactive: a `<span>` showing the value.
- Editing: an `<input>` bound to the value.

## Interactions

### Keyboard

- `ArrowUp/Down/Left/Right` — move focus to adjacent cell (roving `tabindex`).
- `Enter` — start editing focused cell; press again to save and move down one row.
- `Escape` — cancel edit and return focus to the cell.
- `Tab` — leaves the grid to the next page focusable element (standard ARIA grid behavior).
- `Ctrl/Cmd + ArrowUp/Down` — jump to first/last row.

### Drag-to-Reorder

- Pointer-based drag on the row handle.
- Visual feedback: semi-transparent ghost row + a drop indicator line between rows.
- On drop, swap rows in the bound `rows` array.

### Add/Remove

- “+ New row” appends an empty row and moves focus to its first cell.
- “+ New column” appends a column with a generated key and moves focus to its header.
- Row deletion is exposed through a per-row “×” button (visible on focus/hover) to avoid accidental data loss from a single Backspace press.

### Accessibility

- Root grid: `role="grid"`, `aria-label="Sell sheet"`.
- Headers: `role="columnheader"`.
- Body cells: `role="gridcell"`.
- Roving `tabindex` so only one cell is in the tab order.
- `aria-live="polite"` region to announce structural changes (“Row added”, “Column added”).
- Minimum 44×44 px touch targets for drag handles and add buttons.
- Visible focus ring using `--ss-focus-ring` with ≥3:1 contrast.

## Styling

### Tailwind + CSS Variables

Internal components use Tailwind utility classes. Colors, fonts, spacing, and radii are pulled from CSS variables so the default theme can be overridden without rewriting classes.

```css
:root {
  --ss-bg: #fafaf8;
  --ss-cell-bg: #ffffff;
  --ss-border: #e8e6e1;
  --ss-text: #2d2a26;
  --ss-muted: #8a8279;
  --ss-accent: #f4b400;
  --ss-focus-ring: #2d7ff9;
  --ss-radius: 6px;
  --ss-font-ui: 'Sora', sans-serif;
  --ss-font-cell: 'JetBrains Mono', monospace;
}
```

Example usage in components:

```svelte
<div class="bg-[var(--ss-bg)] border border-[var(--ss-border)] rounded-[var(--ss-radius)] font-[family-name:var(--ss-font-ui)]">
```

### Override Path

```svelte
<Sellsheet
  bind:columns
  bind:rows
  class="shadow-xl rounded-xl"
  cellClass="text-sm"
/>
```

## Build & Distribution

- Build tool: `@sveltejs/package` (`svelte-package`).
- `package.json` exports:
  ```json
  {
    "type": "module",
    "exports": {
      ".": {
        "types": "./dist/index.d.ts",
        "svelte": "./dist/index.js"
      },
      "./styles": "./dist/styles.css"
    },
    "files": ["dist"]
  }
  ```
- `dist/` contains compiled Svelte sources, type declarations, and the CSS variable sheet.
- The preview app in `src/routes/` is **not** published.

## SvelteKit Consumer Setup

1. Install the package.
2. Import the component and styles.
3. Add the package source to the consumer’s `tailwind.config.content` so Tailwind scans the library’s utility classes:
   ```js
   content: [
     './src/**/*.{html,js,svelte,ts}',
     './node_modules/@sellsheet/core/src/lib/**/*.{svelte,ts}'
   ]
   ```

## Browser Preview

- `npm run dev` starts the SvelteKit preview at `src/routes/+page.svelte`.
- Preview includes:
  - A default product list (item, price, quantity).
  - A “Reset demo” button.
  - A live JSON panel showing the current `rows` state so `bind:` updates are visible.

## Testing

- **Unit**: Vitest + `@testing-library/svelte` for rendering, prop updates, add/remove helpers.
- **Interaction**: keyboard navigation and add/delete behavior tested via `@testing-library/user-event`.
- **Accessibility**: `axe-core` smoke test against the preview page HTML.
- **Visual/preview**: `src/routes/+page.svelte` is the primary manual verification surface. Playwright drag-to-reorder tests can be added once the interaction stabilizes.

## Open Questions

- Should the package name be `@sellsheet/core` or something else? (Resolved at implementation time based on npm/org availability.)
- Should drag-to-reorder use native HTML5 DnD or pointer events? Decision: pointer-based custom drag for smoother Notion-like feedback.
