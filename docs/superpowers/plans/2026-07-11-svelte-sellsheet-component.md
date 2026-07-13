# SellSheet Component — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Scaffold a Svelte 5 package that exports an accessible, Tailwind-styled `Sellsheet` component, previews it in a browser, and is importable into a SvelteKit app.

**Architecture:** The component is built inside a SvelteKit package skeleton (`@sveltejs/package`). `src/lib/` contains the published component and helpers; `src/routes/` contains a live SvelteKit preview page. State lives in the consumer via `bind:columns` / `bind:rows`. Internal files are split by responsibility: `Cell`, `BodyRow`, `HeaderRow`, `Sellsheet`, and small action modules for drag/keyboard behavior.

**Tech Stack:** Svelte 5, TypeScript, `@sveltejs/kit` + `@sveltejs/package`, Vite, Vitest, `@testing-library/svelte`, Tailwind CSS v3, HTML5 pointer events for drag-to-reorder.

## Global Constraints

- Target: **Svelte 5 (runes)** with TypeScript.
- Package name placeholder: `@sellsheet/core` (resolve npm/org availability before publish).
- All source lives under `sellsheet-component/`; it must not break the parent Rust workspace.
- Tailwind classes reference CSS variables for theming; no hardcoded colors in component markup except via variables.
- Accessibility is required: ARIA `grid` roles, roving tabindex, visible focus ring, labelled buttons.
- Tests are written before or alongside implementation (TDD where practical).
- Each task ends with a working, testable deliverable and a commit.

---

## File Structure

```
sellsheet-component/
├── package.json
├── svelte.config.js
├── vite.config.ts
├── tsconfig.json
├── tailwind.config.js
├── postcss.config.js
├── .gitignore
├── README.md
├── src/
│   ├── app.html
│   ├── app.css
│   ├── lib/
│   │   ├── index.ts
│   │   ├── types.ts
│   │   ├── styles.css
│   │   ├── utils/
│   │   │   └── id.ts
│   │   ├── actions/
│   │   │   ├── keyboardNav.ts
│   │   │   └── dragReorder.ts
│   │   ├── components/
│   │   │   ├── Cell.svelte
│   │   │   ├── BodyRow.svelte
│   │   │   └── HeaderRow.svelte
│   │   └── Sellsheet.svelte
│   └── routes/
│       ├── +layout.svelte
│       └── +page.svelte
└── src/lib/__tests__/
    ├── types.test.ts
    ├── utils/id.test.ts
    ├── components/Cell.svelte.test.ts
    ├── components/HeaderRow.svelte.test.ts
    ├── components/BodyRow.svelte.test.ts
    └── Sellsheet.svelte.test.ts
```

---

### Task 1: Scaffold the SvelteKit package skeleton

**Files:**
- Create: `sellsheet-component/package.json`
- Create: `sellsheet-component/svelte.config.js`
- Create: `sellsheet-component/vite.config.ts`
- Create: `sellsheet-component/tsconfig.json`
- Create: `sellsheet-component/.gitignore`
- Create: `sellsheet-component/src/app.html`

**Interfaces:**
- Produces: a working SvelteKit package dev server at `npm run dev`.

- [ ] **Step 1: Create `package.json`**

```json
{
  "name": "@sellsheet/core",
  "version": "0.0.1",
  "type": "module",
  "scripts": {
    "dev": "vite dev",
    "build": "vite build && npm run package",
    "preview": "vite preview",
    "package": "svelte-kit sync && svelte-package && publint",
    "check": "svelte-kit sync && svelte-check --tsconfig ./tsconfig.json",
    "test": "vitest run",
    "test:watch": "vitest"
  },
  "exports": {
    ".": {
      "types": "./dist/index.d.ts",
      "svelte": "./dist/index.js"
    },
    "./styles": "./dist/styles.css"
  },
  "files": [
    "dist"
  ],
  "peerDependencies": {
    "svelte": "^5.0.0"
  },
  "devDependencies": {
    "@sveltejs/adapter-auto": "^3.0.0",
    "@sveltejs/kit": "^2.0.0",
    "@sveltejs/package": "^2.0.0",
    "@sveltejs/vite-plugin-svelte": "^4.0.0",
    "@testing-library/jest-dom": "^6.4.0",
    "@testing-library/svelte": "^5.2.0",
    "@testing-library/user-event": "^14.5.0",
    "autoprefixer": "^10.4.0",
    "jsdom": "^24.0.0",
    "postcss": "^8.4.0",
    "publint": "^0.2.0",
    "svelte": "^5.0.0",
    "svelte-check": "^4.0.0",
    "tailwindcss": "^3.4.0",
    "typescript": "^5.5.0",
    "vite": "^5.4.0",
    "vitest": "^2.0.0"
  }
}
```

- [ ] **Step 2: Create `svelte.config.js`**

```js
import adapter from '@sveltejs/adapter-auto';
import { vitePreprocess } from '@sveltejs/vite-plugin-svelte';

/** @type {import('@sveltejs/kit').Config} */
const config = {
  preprocess: vitePreprocess(),
  kit: {
    adapter: adapter()
  }
};

export default config;
```

- [ ] **Step 3: Create `vite.config.ts`**

```ts
import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vitest/config';

export default defineConfig({
  plugins: [sveltekit()],
  test: {
    include: ['src/**/*.{test,spec}.{js,ts}'],
    environment: 'jsdom',
    globals: true,
    setupFiles: ['./src/lib/__tests__/setup.ts']
  }
});
```

- [ ] **Step 4: Create `tsconfig.json`**

```json
{
  "extends": "./.svelte-kit/tsconfig.json",
  "compilerOptions": {
    "allowJs": true,
    "checkJs": true,
    "esModuleInterop": true,
    "forceConsistentCasingInFileNames": true,
    "resolveJsonModule": true,
    "skipLibCheck": true,
    "sourceMap": true,
    "strict": true,
    "moduleResolution": "bundler"
  }
}
```

- [ ] **Step 5: Create `.gitignore`**

```gitignore
.DS_Store
node_modules
/.svelte-kit
/package
.env
.env.*
!.env.example
vite.config.ts.timestamp-*
```

- [ ] **Step 6: Create `src/app.html`**

```html
<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <link rel="icon" href="%sveltekit.assets%/favicon.png" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    %sveltekit.head%
  </head>
  <body data-sveltekit-preload-data="hover">
    <div style="display: contents">%sveltekit.body%</div>
  </body>
</html>
```

- [ ] **Step 7: Run install and verify dev server starts**

```bash
cd /Users/chrisarter/Documents/projects/ironlint/sellsheet-component
npm install
npm run check
```

Expected: `svelte-check` runs with no source files and exits 0.

- [ ] **Step 8: Commit**

```bash
git -C /Users/chrisarter/Documents/projects/ironlint add sellsheet-component/
git -C /Users/chrisarter/Documents/projects/ironlint commit -m "chore: scaffold SvelteKit package skeleton"
```

---

### Task 2: Configure Tailwind CSS and global styles

**Files:**
- Create: `sellsheet-component/tailwind.config.js`
- Create: `sellsheet-component/postcss.config.js`
- Create: `sellsheet-component/src/app.css`
- Create: `sellsheet-component/src/lib/styles.css`
- Create: `sellsheet-component/src/routes/+layout.svelte`
- Create: `sellsheet-component/src/routes/+page.svelte` (stub; Task 10 replaces it with the live Sellsheet preview)

**Interfaces:**
- Produces: CSS variables (`--ss-*`) and Tailwind directives available in preview and published styles.

- [ ] **Step 1: Create `tailwind.config.js`**

```js
/** @type {import('tailwindcss').Config} */
export default {
  content: ['./src/**/*.{html,js,svelte,ts}'],
  theme: {
    extend: {}
  },
  plugins: []
};
```

- [ ] **Step 2: Create `postcss.config.js`**

```js
export default {
  plugins: {
    tailwindcss: {},
    autoprefixer: {}
  }
};
```

- [ ] **Step 3: Create `src/app.css`**

```css
@import url('https://fonts.googleapis.com/css2?family=Sora:wght@400;500;600&family=JetBrains+Mono:wght@400;500&display=swap');

@tailwind base;
@tailwind components;
@tailwind utilities;

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

body {
  background-color: var(--ss-bg);
  color: var(--ss-text);
  font-family: var(--ss-font-ui);
}
```

- [ ] **Step 4: Create `src/lib/styles.css`**

```css
@tailwind base;
@tailwind components;
@tailwind utilities;

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

- [ ] **Step 5: Create `src/routes/+layout.svelte` and `+page.svelte`**

`+layout.svelte` (imports the global stylesheet):

```svelte
<script lang="ts">
  import '../app.css';
</script>

<slot />
```

`+page.svelte` (a stub so `/` returns HTTP 200; Task 10 replaces it with the live Sellsheet preview):

```svelte
<h1 class="p-8 text-xl">SellSheet — preview pending (Task 10).</h1>
```

- [ ] **Step 6: Verify Tailwind compiles**

```bash
cd /Users/chrisarter/Documents/projects/ironlint/sellsheet-component
npm run dev
```

Open `http://localhost:5173`. Expected: the stub page loads (HTTP 200) with no Tailwind/Vite errors. Note: `@import url(...)` in `app.css` must precede the `@tailwind` directives — PostCSS rejects an `@import` that follows them (`@import must precede all other statements`). Also run `npm run check`; expect 0 errors / 0 warnings.

- [ ] **Step 7: Commit**

```bash
git -C /Users/chrisarter/Documents/projects/ironlint add sellsheet-component/
git -C /Users/chrisarter/Documents/projects/ironlint commit -m "chore: add Tailwind CSS and global styles"
```

---

### Task 3: Define shared types and id utility

**Files:**
- Create: `sellsheet-component/src/lib/types.ts`
- Create: `sellsheet-component/src/lib/utils/id.ts`
- Create: `sellsheet-component/src/lib/__tests__/utils/id.test.ts`
- Create: `sellsheet-component/src/lib/__tests__/setup.ts`

**Interfaces:**
- Produces: `Column`, `Row`, `generateId()` used by all components.

- [ ] **Step 1: Write the failing test**

Create `src/lib/__tests__/utils/id.test.ts`:

```ts
import { describe, it, expect } from 'vitest';
import { generateId } from '../../utils/id';

describe('generateId', () => {
  it('returns a non-empty string', () => {
    const id = generateId();
    expect(typeof id).toBe('string');
    expect(id.length).toBeGreaterThan(0);
  });

  it('returns unique values', () => {
    const a = generateId();
    const b = generateId();
    expect(a).not.toBe(b);
  });
});
```

Create `src/lib/__tests__/setup.ts`:

```ts
import '@testing-library/jest-dom/vitest';
```

- [ ] **Step 2: Run the failing test**

```bash
cd /Users/chrisarter/Documents/projects/ironlint/sellsheet-component
npm test
```

Expected: `Error: Cannot find module '../../utils/id'`.

- [ ] **Step 3: Implement `types.ts` and `id.ts`**

Create `src/lib/types.ts`:

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

Create `src/lib/utils/id.ts`:

```ts
let counter = 0;

export function generateId(): string {
  counter += 1;
  return `ss_${Date.now().toString(36)}_${counter.toString(36)}`;
}
```

- [ ] **Step 4: Run tests**

```bash
npm test
```

Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
git -C /Users/chrisarter/Documents/projects/ironlint add sellsheet-component/
git -C /Users/chrisarter/Documents/projects/ironlint commit -m "feat: add shared types and id utility"
```

---

### Task 4: Build the `Cell` component

**Files:**
- Create: `sellsheet-component/src/lib/components/Cell.svelte`
- Create: `sellsheet-component/src/lib/__tests__/components/Cell.svelte.test.ts`

**Interfaces:**
- Consumes: `value: string`, `rowIndex: number`, `columnKey: string`, `active: boolean`, `editing: boolean`, `cellClass?: string`.
- Produces: `onfocus`, `onactivate`, `oneditstart`, `oneditend(value)`, `oneditcancel` dispatched events.

- [ ] **Step 1: Write the failing test**

Create `src/lib/__tests__/components/Cell.svelte.test.ts`:

```ts
import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/svelte';
import Cell from '../../components/Cell.svelte';

describe('Cell', () => {
  it('renders the value and gridcell role', () => {
    render(Cell, { props: { value: 'Hello', rowIndex: 0, columnKey: 'name', active: false, editing: false } });
    const cell = screen.getByRole('gridcell');
    expect(cell).toHaveTextContent('Hello');
  });

  it('dispatches activate on click', async () => {
    const onactivate = vi.fn();
    const { component } = render(Cell, { props: { value: 'A', rowIndex: 0, columnKey: 'name', active: false, editing: false } });
    component.$on('activate', onactivate);
    const cell = screen.getByRole('gridcell');
    await fireEvent.click(cell);
    expect(onactivate).toHaveBeenCalledOnce();
  });

  it('switches to input when editing', async () => {
    const { rerender } = render(Cell, { props: { value: 'A', rowIndex: 0, columnKey: 'name', active: true, editing: false } });
    expect(screen.queryByRole('textbox')).toBeNull();
    await rerender({ editing: true });
    expect(screen.getByRole('textbox')).toHaveValue('A');
  });
});
```

- [ ] **Step 2: Run the failing test**

```bash
npm test
```

Expected: `Cannot find module '../../components/Cell.svelte'`.

- [ ] **Step 3: Implement `Cell.svelte`**

```svelte
<script lang="ts">
  import { createEventDispatcher } from 'svelte';

  interface Props {
    value: string;
    rowIndex: number;
    columnKey: string;
    active: boolean;
    editing: boolean;
    cellClass?: string;
  }

  let { value = $bindable(), rowIndex, columnKey, active, editing, cellClass = '' }: Props = $props();

  const dispatch = createEventDispatcher<{
    focus: { rowIndex: number; columnKey: string };
    activate: { rowIndex: number; columnKey: string };
    editstart: { rowIndex: number; columnKey: string };
    editend: { rowIndex: number; columnKey: string; value: string };
    editcancel: { rowIndex: number; columnKey: string };
  }>();

  let inputEl: HTMLInputElement | undefined = $state();

  $effect(() => {
    if (editing && inputEl) {
      inputEl.focus();
      inputEl.select();
    }
  });

  function startEdit() {
    dispatch('editstart', { rowIndex, columnKey });
  }

  function commit() {
    dispatch('editend', { rowIndex, columnKey, value });
  }

  function cancel() {
    dispatch('editcancel', { rowIndex, columnKey });
  }

  function onKeyDown(e: KeyboardEvent) {
    if (e.key === 'Enter') {
      e.preventDefault();
      commit();
    } else if (e.key === 'Escape') {
      e.preventDefault();
      cancel();
    }
  }

  function onClick() {
    dispatch('activate', { rowIndex, columnKey });
  }
</script>

{#if editing}
  <input
    bind:this={inputEl}
    bind:value
    role="gridcell"
    aria-rowindex={rowIndex + 1}
    aria-colindex={columnKey}
    tabindex={active ? 0 : -1}
    class="{cellClass} w-full bg-[var(--ss-cell-bg)] text-[var(--ss-text)] font-[family-name:var(--ss-font-cell)] px-3 py-2 outline-none ring-2 ring-[var(--ss-focus-ring)] rounded-[var(--ss-radius)]"
    onkeydown={onKeyDown}
    onblur={commit}
  />
{:else}
  <span
    role="gridcell"
    aria-rowindex={rowIndex + 1}
    aria-colindex={columnKey}
    tabindex={active ? 0 : -1}
    class="{cellClass} block px-3 py-2 min-h-[44px] cursor-text select-none rounded-[var(--ss-radius)] hover:bg-[var(--ss-border)]/30 focus-visible:ring-2 focus-visible:ring-[var(--ss-focus-ring)]"
    onclick={onClick}
    ondblclick={startEdit}
  >
    {value}
  </span>
{/if}
```

- [ ] **Step 4: Run tests**

```bash
npm test
```

Expected: all Cell tests pass.

- [ ] **Step 5: Commit**

```bash
git -C /Users/chrisarter/Documents/projects/ironlint add sellsheet-component/
git -C /Users/chrisarter/Documents/projects/ironlint commit -m "feat: add Cell component with inline editing"
```

---

### Task 5: Build the `HeaderRow` component

**Files:**
- Create: `sellsheet-component/src/lib/components/HeaderRow.svelte`
- Create: `sellsheet-component/src/lib/__tests__/components/HeaderRow.svelte.test.ts`

**Interfaces:**
- Consumes: `columns: Column[]`, `activeColumnKey?: string`, `headerClass?: string`.
- Produces: `onaddcolumn`, `onfocuscolumn` events.

- [ ] **Step 1: Write the failing test**

Create `src/lib/__tests__/components/HeaderRow.svelte.test.ts`:

```ts
import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/svelte';
import HeaderRow from '../../components/HeaderRow.svelte';

describe('HeaderRow', () => {
  const columns = [
    { key: 'item', label: 'Item' },
    { key: 'price', label: 'Price' }
  ];

  it('renders column headers', () => {
    render(HeaderRow, { props: { columns } });
    expect(screen.getByRole('columnheader', { name: 'Item' })).toBeInTheDocument();
    expect(screen.getByRole('columnheader', { name: 'Price' })).toBeInTheDocument();
  });

  it('dispatches addcolumn when add button is clicked', async () => {
    const onaddcolumn = vi.fn();
    const { component } = render(HeaderRow, { props: { columns } });
    component.$on('addcolumn', onaddcolumn);
    const btn = screen.getByRole('button', { name: /add new column/i });
    await fireEvent.click(btn);
    expect(onaddcolumn).toHaveBeenCalledOnce();
  });
});
```

- [ ] **Step 2: Run the failing test**

```bash
npm test
```

Expected: module not found.

- [ ] **Step 3: Implement `HeaderRow.svelte`**

```svelte
<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import type { Column } from '../types';

  interface Props {
    columns: Column[];
    activeColumnKey?: string;
    headerClass?: string;
  }

  let { columns, activeColumnKey, headerClass = '' }: Props = $props();

  const dispatch = createEventDispatcher<{
    addcolumn: void;
    focuscolumn: { columnKey: string };
  }>();
</script>

<div role="row" class="flex items-center border-b border-[var(--ss-border)]">
  <div class="w-10 shrink-0" aria-hidden="true"></div>
  {#each columns as column (column.key)}
    <button
      type="button"
      role="columnheader"
      tabindex={activeColumnKey === column.key ? 0 : -1}
      class="{headerClass} flex-1 min-w-[120px] text-left px-3 py-2 text-sm font-medium text-[var(--ss-muted)] font-[family-name:var(--ss-font-ui)] hover:bg-[var(--ss-border)]/30 focus-visible:ring-2 focus-visible:ring-[var(--ss-focus-ring)]"
      onclick={() => dispatch('focuscolumn', { columnKey: column.key })}
    >
      {column.label}
    </button>
  {/each}
  <button
    type="button"
    aria-label="Add new column"
    class="ml-2 px-3 py-2 text-sm text-[var(--ss-muted)] hover:text-[var(--ss-text)] focus-visible:ring-2 focus-visible:ring-[var(--ss-focus-ring)] rounded-[var(--ss-radius)]"
    onclick={() => dispatch('addcolumn')}
  >
    + New column
  </button>
</div>
```

- [ ] **Step 4: Run tests**

```bash
npm test
```

Expected: HeaderRow tests pass.

- [ ] **Step 5: Commit**

```bash
git -C /Users/chrisarter/Documents/projects/ironlint add sellsheet-component/
git -C /Users/chrisarter/Documents/projects/ironlint commit -m "feat: add HeaderRow component"
```

---

### Task 6: Build the `BodyRow` component

**Files:**
- Create: `sellsheet-component/src/lib/components/BodyRow.svelte`
- Create: `sellsheet-component/src/lib/__tests__/components/BodyRow.svelte.test.ts`

**Interfaces:**
- Consumes: `columns`, `row`, `rowIndex`, `activeCell`, `editingCell`, `rowClass`, `cellClass`.
- Produces: cell events forwarded upward; `ondeleterow`, `ondragstart`, `ondragend` events.

- [ ] **Step 1: Write the failing test**

Create `src/lib/__tests__/components/BodyRow.svelte.test.ts`:

```ts
import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/svelte';
import BodyRow from '../../components/BodyRow.svelte';

describe('BodyRow', () => {
  const columns = [
    { key: 'item', label: 'Item' },
    { key: 'price', label: 'Price' }
  ];
  const row = { id: 'r1', item: 'Camera', price: '120' };

  it('renders cells for each column', () => {
    render(BodyRow, { props: { columns, row, rowIndex: 0, activeCell: null, editingCell: null } });
    expect(screen.getByRole('gridcell', { name: 'Camera' })).toBeInTheDocument();
    expect(screen.getByRole('gridcell', { name: '120' })).toBeInTheDocument();
  });

  it('dispatches deleterow when delete button clicked', async () => {
    const ondeleterow = vi.fn();
    const { component } = render(BodyRow, { props: { columns, row, rowIndex: 0, activeCell: null, editingCell: null } });
    component.$on('deleterow', ondeleterow);
    const btn = screen.getByRole('button', { name: /delete row/i });
    await fireEvent.click(btn);
    expect(ondeleterow).toHaveBeenCalledWith(expect.objectContaining({ detail: { rowIndex: 0 } }));
  });
});
```

- [ ] **Step 2: Run the failing test**

```bash
npm test
```

Expected: module not found.

- [ ] **Step 3: Implement `BodyRow.svelte`**

```svelte
<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import type { Column, Row } from '../types';
  import Cell from './Cell.svelte';

  interface Props {
    columns: Column[];
    row: Row;
    rowIndex: number;
    activeCell: { rowIndex: number; columnKey: string } | null;
    editingCell: { rowIndex: number; columnKey: string } | null;
    rowClass?: string;
    cellClass?: string;
  }

  let { columns, row = $bindable(), rowIndex, activeCell, editingCell, rowClass = '', cellClass = '' }: Props = $props();

  const dispatch = createEventDispatcher<{
    cellfocus: { rowIndex: number; columnKey: string };
    celleditstart: { rowIndex: number; columnKey: string };
    celleditend: { rowIndex: number; columnKey: string; value: string };
    celleditcancel: { rowIndex: number; columnKey: string };
    deleterow: { rowIndex: number };
    dragstart: { rowIndex: number };
    dragend: { from: number; to: number };
  }>();

  function isActive(columnKey: string) {
    return activeCell?.rowIndex === rowIndex && activeCell?.columnKey === columnKey;
  }

  function isEditing(columnKey: string) {
    return editingCell?.rowIndex === rowIndex && editingCell?.columnKey === columnKey;
  }
</script>

<div role="row" class="{rowClass} group flex items-center border-b border-[var(--ss-border)] hover:bg-[var(--ss-border)]/20">
  <div class="w-10 shrink-0 flex items-center justify-center">
    <button
      type="button"
      aria-label="Drag to reorder row {rowIndex + 1}"
      class="cursor-grab active:cursor-grabbing p-2 text-[var(--ss-muted)] opacity-0 group-hover:opacity-100 focus:opacity-100 focus-visible:ring-2 focus-visible:ring-[var(--ss-focus-ring)]"
    >
      ⋮⋮
    </button>
  </div>
  {#each columns as column (column.key)}
    <Cell
      bind:value={row[column.key]}
      {rowIndex}
      columnKey={column.key}
      active={isActive(column.key)}
      editing={isEditing(column.key)}
      {cellClass}
      on:focus={(e) => dispatch('cellfocus', e.detail)}
      on:activate={(e) => dispatch('cellfocus', e.detail)}
      on:editstart={(e) => dispatch('celleditstart', e.detail)}
      on:editend={(e) => dispatch('celleditend', e.detail)}
      on:editcancel={(e) => dispatch('celleditcancel', e.detail)}
    />
  {/each}
  <button
    type="button"
    aria-label="Delete row {rowIndex + 1}"
    class="ml-auto px-3 py-2 text-[var(--ss-muted)] opacity-0 group-hover:opacity-100 focus:opacity-100 focus-visible:ring-2 focus-visible:ring-[var(--ss-focus-ring)] rounded-[var(--ss-radius)]"
    onclick={() => dispatch('deleterow', { rowIndex })}
  >
    ×
  </button>
</div>
```

- [ ] **Step 4: Run tests**

```bash
npm test
```

Expected: BodyRow tests pass.

- [ ] **Step 5: Commit**

```bash
git -C /Users/chrisarter/Documents/projects/ironlint add sellsheet-component/
git -C /Users/chrisarter/Documents/projects/ironlint commit -m "feat: add BodyRow component"
```

---

### Task 7: Implement keyboard navigation action

**Files:**
- Create: `sellsheet-component/src/lib/actions/keyboardNav.ts`
- Create: `sellsheet-component/src/lib/__tests__/actions/keyboardNav.test.ts`

**Interfaces:**
- Produces: `handleKeyDown` function consumed by `Sellsheet.svelte`.

- [ ] **Step 1: Write the failing test**

Create `src/lib/__tests__/actions/keyboardNav.test.ts`:

```ts
import { describe, it, expect } from 'vitest';
import { nextCell } from '../../actions/keyboardNav';

describe('nextCell', () => {
  const columns = [{ key: 'a' }, { key: 'b' }] as const;
  const rows = [{ id: '1' }, { id: '2' }];

  it('moves right', () => {
    expect(nextCell({ rowIndex: 0, columnKey: 'a' }, 'ArrowRight', rows, columns as any)).toEqual({ rowIndex: 0, columnKey: 'b' });
  });

  it('moves down and wraps to next row', () => {
    expect(nextCell({ rowIndex: 0, columnKey: 'b' }, 'ArrowRight', rows, columns as any)).toEqual({ rowIndex: 1, columnKey: 'a' });
  });

  it('moves up', () => {
    expect(nextCell({ rowIndex: 1, columnKey: 'a' }, 'ArrowUp', rows, columns as any)).toEqual({ rowIndex: 0, columnKey: 'a' });
  });
});
```

- [ ] **Step 2: Run the failing test**

```bash
npm test
```

Expected: `Cannot find module '../../actions/keyboardNav'`.

- [ ] **Step 3: Implement `keyboardNav.ts`**

```ts
import type { Column, Row } from '../types';

export type CellCoord = { rowIndex: number; columnKey: string };

export function nextCell(
  current: CellCoord,
  key: string,
  rows: Row[],
  columns: Column[],
  editing: boolean
): CellCoord | null {
  const columnIndex = columns.findIndex((c) => c.key === current.columnKey);
  if (columnIndex === -1) return null;

  switch (key) {
    case 'ArrowRight': {
      if (editing) return null;
      const nextIndex = columnIndex + 1;
      if (nextIndex < columns.length) {
        return { rowIndex: current.rowIndex, columnKey: columns[nextIndex].key };
      }
      if (current.rowIndex + 1 < rows.length) {
        return { rowIndex: current.rowIndex + 1, columnKey: columns[0].key };
      }
      return null;
    }
    case 'ArrowLeft': {
      if (editing) return null;
      const prevIndex = columnIndex - 1;
      if (prevIndex >= 0) {
        return { rowIndex: current.rowIndex, columnKey: columns[prevIndex].key };
      }
      if (current.rowIndex - 1 >= 0) {
        return { rowIndex: current.rowIndex - 1, columnKey: columns[columns.length - 1].key };
      }
      return null;
    }
    case 'ArrowDown': {
      if (editing && key !== 'Enter') return null;
      const lastRow = key === 'Enter' ? current.rowIndex : current.rowIndex;
      const targetRow = key === 'Enter' ? lastRow + 1 : current.rowIndex + 1;
      if (targetRow < rows.length) {
        return { rowIndex: targetRow, columnKey: current.columnKey };
      }
      return null;
    }
    case 'ArrowUp': {
      if (editing) return null;
      if (current.rowIndex - 1 >= 0) {
        return { rowIndex: current.rowIndex - 1, columnKey: current.columnKey };
      }
      return null;
    }
    default:
      return null;
  }
}
```

Note: This initial implementation focuses on the four arrow keys. The `Enter` behavior is handled in `Sellsheet.svelte` by calling `nextCell` with a synthetic `'ArrowDown'` after committing.

- [ ] **Step 4: Run tests**

```bash
npm test
```

Expected: keyboardNav tests pass.

- [ ] **Step 5: Commit**

```bash
git -C /Users/chrisarter/Documents/projects/ironlint add sellsheet-component/
git -C /Users/chrisarter/Documents/projects/ironlint commit -m "feat: add keyboard navigation helper"
```

---

### Task 8: Implement drag-to-reorder action

**Files:**
- Create: `sellsheet-component/src/lib/actions/dragReorder.ts`
- Create: `sellsheet-component/src/lib/__tests__/actions/dragReorder.test.ts`

**Interfaces:**
- Produces: Svelte action `dragRow(node, { rowIndex, onDragStart, onDragMove, onDragEnd })`.

- [ ] **Step 1: Write the failing test**

Create `src/lib/__tests__/actions/dragReorder.test.ts`:

```ts
import { describe, it, expect, vi } from 'vitest';
import { render } from '@testing-library/svelte';
import TestDrag from './TestDrag.svelte';

describe('dragReorder', () => {
  it('calls onDragEnd with from and to indices', async () => {
    const onDragEnd = vi.fn();
    const { container } = render(TestDrag, { props: { onDragEnd } });
    const handle = container.querySelector('[data-drag]') as HTMLElement;

    const down = new PointerEvent('pointerdown', { bubbles: true, clientY: 50 });
    handle.dispatchEvent(down);

    const move = new PointerEvent('pointermove', { bubbles: true, clientY: 110 });
    document.dispatchEvent(move);

    const up = new PointerEvent('pointerup', { bubbles: true });
    document.dispatchEvent(up);

    expect(onDragEnd).toHaveBeenCalledWith(0, 1);
  });
});
```

Create `src/lib/__tests__/actions/TestDrag.svelte`:

```svelte
<script lang="ts">
  import { dragRow } from '../../actions/dragReorder';

  interface Props {
    onDragEnd: (from: number, to: number) => void;
  }

  let { onDragEnd }: Props = $props();
</script>

<div class="h-10" data-row data-row-index="0">Row 0</div>
<button data-drag use:dragRow={{ rowIndex: 0, onDragEnd }}>drag</button>
<div class="h-10" data-row data-row-index="1">Row 1</div>
```

- [ ] **Step 2: Run the failing test**

```bash
npm test
```

Expected: `Cannot find module '../../actions/dragReorder'`.

- [ ] **Step 3: Implement `dragReorder.ts`**

```ts
import type { Action } from 'svelte/action';

interface DragRowParams {
  rowIndex: number;
  onDragStart?: (index: number) => void;
  onDragMove?: (from: number, to: number) => void;
  onDragEnd: (from: number, to: number) => void;
}

export const dragRow: Action<HTMLElement, DragRowParams> = (node, params) => {
  let current = params;
  let startY = 0;
  let rowHeight = 0;

  function onPointerDown(e: PointerEvent) {
    e.preventDefault();
    startY = e.clientY;
    const row = node.closest('[data-row]') as HTMLElement | null;
    rowHeight = row?.offsetHeight ?? 40;
    current.onDragStart?.(current.rowIndex);
    node.setPointerCapture(e.pointerId);
    node.addEventListener('pointermove', onPointerMove);
    node.addEventListener('pointerup', onPointerUp);
  }

  function onPointerMove(e: PointerEvent) {
    const delta = e.clientY - startY;
    const steps = Math.round(delta / rowHeight);
    const to = Math.max(0, current.rowIndex + steps);
    current.onDragMove?.(current.rowIndex, to);
  }

  function onPointerUp(e: PointerEvent) {
    const delta = e.clientY - startY;
    const steps = Math.round(delta / rowHeight);
    const to = Math.max(0, current.rowIndex + steps);
    current.onDragEnd(current.rowIndex, to);
    node.releasePointerCapture(e.pointerId);
    node.removeEventListener('pointermove', onPointerMove);
    node.removeEventListener('pointerup', onPointerUp);
  }

  node.addEventListener('pointerdown', onPointerDown);

  return {
    update(newParams) {
      current = newParams;
    },
    destroy() {
      node.removeEventListener('pointerdown', onPointerDown);
      node.removeEventListener('pointermove', onPointerMove);
      node.removeEventListener('pointerup', onPointerUp);
    }
  };
};
```

- [ ] **Step 4: Run tests**

```bash
npm test
```

Expected: dragReorder tests pass.

- [ ] **Step 5: Commit**

```bash
git -C /Users/chrisarter/Documents/projects/ironlint add sellsheet-component/
git -C /Users/chrisarter/Documents/projects/ironlint commit -m "feat: add drag-to-reorder action"
```

---

### Task 9: Assemble the `Sellsheet` root component

**Files:**
- Create: `sellsheet-component/src/lib/Sellsheet.svelte`
- Modify: `sellsheet-component/src/lib/index.ts`
- Create: `sellsheet-component/src/lib/__tests__/Sellsheet.svelte.test.ts`

**Interfaces:**
- Consumes: `columns` (bindable), `rows` (bindable), optional class props.
- Produces: rendered accessible grid with all interactions.

- [ ] **Step 1: Write the failing test**

Create `src/lib/__tests__/Sellsheet.svelte.test.ts`:

```ts
import { describe, it, expect } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/svelte';
import Sellsheet from '../Sellsheet.svelte';

describe('Sellsheet', () => {
  const columns = [
    { key: 'item', label: 'Item' },
    { key: 'price', label: 'Price' }
  ];
  const rows = [
    { id: 'r1', item: 'Camera', price: '120' },
    { id: 'r2', item: 'Lens', price: '250' }
  ];

  it('renders a grid with rows', () => {
    render(Sellsheet, { props: { columns, rows } });
    expect(screen.getByRole('grid')).toBeInTheDocument();
    expect(screen.getAllByRole('row')).toHaveLength(3); // header + 2 rows
  });

  it('adds a row when add-row button is clicked', async () => {
    const { component } = render(Sellsheet, { props: { columns, rows } });
    const btn = screen.getByRole('button', { name: /add new row/i });
    await fireEvent.click(btn);
    expect(component.rows).toHaveLength(3);
  });

  it('adds a column when add-column button is clicked', async () => {
    const { component } = render(Sellsheet, { props: { columns, rows } });
    const btn = screen.getByRole('button', { name: /add new column/i });
    await fireEvent.click(btn);
    expect(component.columns).toHaveLength(3);
  });
});
```

- [ ] **Step 2: Run the failing test**

```bash
npm test
```

Expected: `Cannot find module '../Sellsheet.svelte'`.

- [ ] **Step 3: Implement `Sellsheet.svelte`**

```svelte
<script lang="ts">
  import type { Column, Row } from './types';
  import HeaderRow from './components/HeaderRow.svelte';
  import BodyRow from './components/BodyRow.svelte';
  import { nextCell, type CellCoord } from './actions/keyboardNav';
  import { generateId } from './utils/id';

  interface Props {
    columns: Column[];
    rows: Row[];
    class?: string;
    headerClass?: string;
    rowClass?: string;
    cellClass?: string;
  }

  let {
    columns = $bindable(),
    rows = $bindable(),
    class: className = '',
    headerClass = '',
    rowClass = '',
    cellClass = ''
  }: Props = $props();

  let activeCell: CellCoord | null = $state(rows.length > 0 && columns.length > 0 ? { rowIndex: 0, columnKey: columns[0].key } : null);
  let editingCell: CellCoord | null = $state(null);

  function addColumn() {
    const key = `col_${generateId()}`;
    columns = [...columns, { key, label: 'New column' }];
    activeCell = null;
  }

  function addRow() {
    const newRow: Row = { id: generateId() };
    for (const col of columns) {
      newRow[col.key] = '';
    }
    rows = [...rows, newRow];
    activeCell = { rowIndex: rows.length - 1, columnKey: columns[0]?.key ?? '' };
  }

  function deleteRow(rowIndex: number) {
    rows = rows.filter((_, i) => i !== rowIndex);
    if (activeCell?.rowIndex === rowIndex) {
      activeCell = null;
    }
  }

  function onCellFocus(coord: CellCoord) {
    activeCell = coord;
    editingCell = null;
  }

  function onEditStart(coord: CellCoord) {
    activeCell = coord;
    editingCell = coord;
  }

  function onEditEnd(coord: CellCoord & { value: string }) {
    rows = rows.map((row, i) => (i === coord.rowIndex ? { ...row, [coord.columnKey]: coord.value } : row));
    editingCell = null;
    const next = nextCell(coord, 'ArrowDown', rows, columns, false);
    if (next) activeCell = next;
  }

  function onEditCancel(coord: CellCoord) {
    editingCell = null;
    activeCell = coord;
  }

  function onKeyDown(e: KeyboardEvent) {
    if (!activeCell) return;
    if (e.key === 'Enter') {
      e.preventDefault();
      if (editingCell) {
        onEditEnd({ ...editingCell, value: rows[editingCell.rowIndex][editingCell.columnKey] });
      } else {
        onEditStart(activeCell);
      }
      return;
    }
    if (e.key === 'Escape' && editingCell) {
      e.preventDefault();
      onEditCancel(editingCell);
      return;
    }
    const next = nextCell(activeCell, e.key, rows, columns, !!editingCell);
    if (next) {
      e.preventDefault();
      activeCell = next;
      editingCell = null;
    }
  }

  function onDragEnd(from: number, to: number) {
    if (from === to) return;
    const item = rows[from];
    const reordered = [...rows];
    reordered.splice(from, 1);
    reordered.splice(to, 0, item);
    rows = reordered;
  }
</script>

<div
  role="grid"
  aria-label="Sell sheet"
  class="{className} inline-block min-w-full border border-[var(--ss-border)] rounded-[var(--ss-radius)] overflow-hidden bg-[var(--ss-cell-bg)]"
  onkeydown={onKeyDown}
>
  <HeaderRow
    {columns}
    activeColumnKey={activeCell?.columnKey}
    {headerClass}
    on:addcolumn={addColumn}
  />
  {#each rows as row, rowIndex (row.id)}
    <BodyRow
      bind:row
      {columns}
      {rowIndex}
      {activeCell}
      {editingCell}
      {rowClass}
      {cellClass}
      on:cellfocus={(e) => onCellFocus(e.detail)}
      on:celleditstart={(e) => onEditStart(e.detail)}
      on:celleditend={(e) => onEditEnd(e.detail)}
      on:celleditcancel={(e) => onEditCancel(e.detail)}
      on:deleterow={(e) => deleteRow(e.detail.rowIndex)}
      on:dragend={(e) => onDragEnd(e.detail.from, e.detail.to)}
    />
  {/each}
  <button
    type="button"
    aria-label="Add new row"
    class="w-full px-4 py-2 text-left text-sm text-[var(--ss-muted)] hover:bg-[var(--ss-border)]/30 hover:text-[var(--ss-text)] focus-visible:ring-2 focus-visible:ring-[var(--ss-focus-ring)]"
    onclick={addRow}
  >
    + New row
  </button>
</div>
```

- [ ] **Step 4: Implement `index.ts`**

```ts
export { default as default } from './Sellsheet.svelte';
export type { Column, Row } from './types';
```

- [ ] **Step 5: Run tests**

```bash
npm test
```

Expected: Sellsheet tests pass.

- [ ] **Step 6: Commit**

```bash
git -C /Users/chrisarter/Documents/projects/ironlint add sellsheet-component/
git -C /Users/chrisarter/Documents/projects/ironlint commit -m "feat: assemble Sellsheet root component"
```

---

### Task 10: Wire up the browser preview page

**Files:**
- Modify: `sellsheet-component/src/routes/+page.svelte`
- Create: `sellsheet-component/static/favicon.png` (optional placeholder)

**Interfaces:**
- Consumes: `Sellsheet` component, `Column`, `Row` types.
- Produces: live browser preview at `npm run dev`.

- [ ] **Step 1: Create the preview page**

Modify `src/routes/+page.svelte`:

```svelte
<script lang="ts">
  import Sellsheet from '$lib/Sellsheet.svelte';
  import type { Column, Row } from '$lib/types';

  let columns: Column[] = $state([
    { key: 'item', label: 'Item' },
    { key: 'price', label: 'Price' },
    { key: 'quantity', label: 'Qty' }
  ]);

  let rows: Row[] = $state([
    { id: 'r1', item: 'Vintage camera', price: '120', quantity: '1' },
    { id: 'r2', item: 'Leather strap', price: '35', quantity: '2' },
    { id: 'r3', item: 'Lens cap', price: '8', quantity: '5' }
  ]);

  function reset() {
    rows = [
      { id: 'r1', item: 'Vintage camera', price: '120', quantity: '1' },
      { id: 'r2', item: 'Leather strap', price: '35', quantity: '2' },
      { id: 'r3', item: 'Lens cap', price: '8', quantity: '5' }
    ];
  }
</script>

<main class="max-w-4xl mx-auto p-8">
  <header class="mb-6">
    <h1 class="text-2xl font-semibold font-[family-name:var(--ss-font-ui)]">SellSheet Preview</h1>
    <p class="text-[var(--ss-muted)] text-sm mt-1">Try Tab, Enter, Arrows, and drag handles.</p>
  </header>

  <section class="mb-6">
    <Sellsheet bind:columns bind:rows />
  </section>

  <section class="flex gap-4 mb-6">
    <button
      type="button"
      class="px-4 py-2 rounded-[var(--ss-radius)] bg-[var(--ss-text)] text-[var(--ss-cell-bg)] hover:opacity-90"
      onclick={reset}
    >
      Reset demo
    </button>
  </section>

  <section class="p-4 rounded-[var(--ss-radius)] bg-[var(--ss-bg)] border border-[var(--ss-border)]">
    <h2 class="text-sm font-medium mb-2 text-[var(--ss-muted)]">Live rows</h2>
    <pre class="text-xs overflow-auto font-[family-name:var(--ss-font-cell)]">{JSON.stringify(rows, null, 2)}</pre>
  </section>
</main>
```

- [ ] **Step 2: Start the dev server and verify**

```bash
cd /Users/chrisarter/Documents/projects/ironlint/sellsheet-component
npm run dev
```

Open `http://localhost:5173`. Expected:
- Three rows and three columns render.
- “+ New row” and “+ New column” buttons are visible.
- Clicking a cell focuses it; Enter starts editing; Tab moves focus out of grid; Arrows move between cells.
- The “Live rows” JSON updates when you edit cells or add rows.

- [ ] **Step 3: Commit**

```bash
git -C /Users/chrisarter/Documents/projects/ironlint add sellsheet-component/
git -C /Users/chrisarter/Documents/projects/ironlint commit -m "feat: add browser preview page"
```

---

### Task 11: Package build and SvelteKit importability verification

**Files:**
- Modify: `sellsheet-component/package.json` (ensure `files`, `exports`, `svelte` conditions are correct).
- Create: temporary `sellsheet-component/.tmp-consumer/` for end-to-end import check.

**Interfaces:**
- Produces: `dist/` folder with component sources, types, and styles.

- [ ] **Step 1: Build the package**

```bash
cd /Users/chrisarter/Documents/projects/ironlint/sellsheet-component
npm run package
```

Expected: `dist/` is created with `index.js`, `index.d.ts`, `styles.css`, and the `.svelte` files.

- [ ] **Step 2: Verify SvelteKit consumer import**

Create a temporary SvelteKit app inside `.tmp-consumer/`:

```bash
mkdir -p .tmp-consumer
```

Inside `.tmp-consumer/package.json`:

```json
{
  "name": "tmp-consumer",
  "version": "0.0.1",
  "type": "module",
  "scripts": {
    "check": "svelte-kit sync && svelte-check --tsconfig ./tsconfig.json"
  },
  "dependencies": {
    "@sellsheet/core": "file:.."
  },
  "devDependencies": {
    "@sveltejs/adapter-auto": "^3.0.0",
    "@sveltejs/kit": "^2.0.0",
    "@sveltejs/vite-plugin-svelte": "^4.0.0",
    "svelte": "^5.0.0",
    "svelte-check": "^4.0.0",
    "typescript": "^5.5.0",
    "vite": "^5.4.0"
  }
}
```

Inside `.tmp-consumer/src/routes/+page.svelte`:

```svelte
<script lang="ts">
  import Sellsheet from '@sellsheet/core';
  import '@sellsheet/core/styles';
  import type { Column, Row } from '@sellsheet/core';

  let columns: Column[] = $state([{ key: 'a', label: 'A' }]);
  let rows: Row[] = $state([{ id: '1', a: 'hello' }]);
</script>

<Sellsheet bind:columns bind:rows />
```

Run:

```bash
cd .tmp-consumer
npm install
npm run check
```

Expected: `svelte-check` passes with no errors.

- [ ] **Step 3: Clean up temporary consumer**

```bash
cd /Users/chrisarter/Documents/projects/ironlint/sellsheet-component
rm -rf .tmp-consumer
```

- [ ] **Step 4: Commit**

```bash
git -C /Users/chrisarter/Documents/projects/ironlint add sellsheet-component/
git -C /Users/chrisarter/Documents/projects/ironlint commit -m "build: verify package exports and SvelteKit importability"
```

---

### Task 12: Accessibility smoke test

**Files:**
- Modify: `sellsheet-component/src/lib/__tests__/Sellsheet.svelte.test.ts`
- Install: `axe-core` as dev dependency.

**Interfaces:**
- Produces: passing a11y assertions on the rendered grid.

- [ ] **Step 1: Install `axe-core`**

```bash
cd /Users/chrisarter/Documents/projects/ironlint/sellsheet-component
npm install -D axe-core @types/axe-core
```

- [ ] **Step 2: Add axe test**

Append to `src/lib/__tests__/Sellsheet.svelte.test.ts`:

```ts
import axe from 'axe-core';

it('has no detectable accessibility violations', async () => {
  const { container } = render(Sellsheet, { props: { columns, rows } });
  const results = await axe.run(container);
  expect(results.violations).toHaveLength(0);
});
```

- [ ] **Step 3: Run tests**

```bash
npm test
```

Expected: all tests pass including the axe smoke test.

- [ ] **Step 4: Commit**

```bash
git -C /Users/chrisarter/Documents/projects/ironlint add sellsheet-component/
git -C /Users/chrisarter/Documents/projects/ironlint commit -m "test: add axe accessibility smoke test"
```

---

### Task 13: Final documentation

**Files:**
- Create: `sellsheet-component/README.md`

**Interfaces:**
- Produces: consumer-facing README with install, usage, Tailwind setup, and API notes.

- [ ] **Step 1: Write `README.md`**

```markdown
# @sellsheet/core

The core spreadsheet-style list component for SellSheet.

## Install

```bash
npm install @sellsheet/core
```

## Usage

```svelte
<script lang="ts">
  import Sellsheet from '@sellsheet/core';
  import '@sellsheet/core/styles';
  import type { Column, Row } from '@sellsheet/core';

  let columns: Column[] = [
    { key: 'item', label: 'Item' },
    { key: 'price', label: 'Price' }
  ];

  let rows: Row[] = [
    { id: 'r1', item: 'Vintage camera', price: '120' }
  ];
</script>

<Sellsheet bind:columns bind:rows />
```

## Tailwind setup

Because the package ships source `.svelte` files, add it to your Tailwind config:

```js
content: [
  './src/**/*.{html,js,svelte,ts}',
  './node_modules/@sellsheet/core/src/lib/**/*.{svelte,ts}'
]
```

## Keyboard shortcuts

- `Arrows` — move between cells
- `Enter` — edit / save and move down
- `Escape` — cancel edit
- `Tab` — leave the grid

## Styling overrides

Pass Tailwind classes via props:

```svelte
<Sellsheet bind:columns bind:rows class="shadow-xl" cellClass="text-sm" />
```

Override CSS variables for theming:

```css
:root {
  --ss-bg: #f8f9fa;
  --ss-focus-ring: #0066cc;
}
```
```

- [ ] **Step 2: Run final checks**

```bash
cd /Users/chrisarter/Documents/projects/ironlint/sellsheet-component
npm run check
npm test
npm run package
```

Expected: `check`, `test`, and `package` all succeed.

- [ ] **Step 3: Commit**

```bash
git -C /Users/chrisarter/Documents/projects/ironlint add sellsheet-component/
git -C /Users/chrisarter/Documents/projects/ironlint commit -m "docs: add README"
```

---

## Self-Review Checklist

- [ ] **Spec coverage:**
  - Svelte 5 package skeleton — Task 1
  - Tailwind + CSS variables — Task 2
  - Dynamic columns/rows API — Tasks 3, 9
  - Plain-text inline editing — Task 4
  - Keyboard navigation (Tab/Enter/Arrows) — Tasks 4, 7, 9
  - Drag-to-reorder — Tasks 6, 8
  - Add/remove row/column — Tasks 5, 9
  - Accessibility (ARIA grid, roving tabindex, labels) — Tasks 4–6, 9, 12
  - Browser preview — Task 10
  - SvelteKit importability — Task 11
  - Styling override props — Task 9
- [ ] **Placeholder scan:** No “TBD”, “TODO”, or vague steps remain.
- [ ] **Type consistency:** `Column`, `Row`, `CellCoord`, and event detail shapes are used consistently across tasks.
