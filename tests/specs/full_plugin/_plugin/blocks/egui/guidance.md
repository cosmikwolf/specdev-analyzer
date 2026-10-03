# egui — implementation guidance

Rules an agent follows when it writes or edits code for a module with an `[egui]` table. The block models topology
only — panels and regions say where things go. No check verifies these rules; review does.

## The host and the egui path

- This block opens no window and creates no egui context. A host does: a native-shell block, a bridge to a plugin
  editor, or the project itself.
- When a host unit is inlined, the crate root re-exports the host's egui once, and every file reaches egui through
  that re-export. Never add a second egui dependency: two hosts in one crate must fail at that one line, not deep in
  mismatched types.
- When no host unit is inlined, the project is the host: it depends on egui directly in `Cargo.toml` and the code
  uses the plain crate path, with no re-export.
- Do not add a host crate or an extras crate from this block's code; those are the project's choices.

## Panels

- One drawing function per module with `[egui]`, with every panel in declaration order.
- egui has two panel API generations. Find the one the host's egui has — by its types, not by memory — before you
  write a panel, and use it throughout. The drawing function's parameter (a `Ui` or a `Context`) follows from it.
- Side, top and bottom panels come before the central panel: each claims its strip, and the central panel fills the
  rest.
- `width` applies to side panels only, `height` to top and bottom panels only.
- `[egui].default_size` and `[egui].resizable` belong to the host, not to this block's code.

## Regions

- Regions appear in declaration order inside their panel, wrapped in the `layout` the spec names (vertical when
  omitted). A grid layout needs `columns`; `columns` on another layout means nothing.
- `parameter_controls`: a bridge fills it with value-bound widgets. With no bridge, the body is hand-written.
- `metering`: the region only reads a value another thread publishes through a shared atomic cell, passed in as a
  parameter. The producer side belongs to the project's profile rules.
- `preset_selector`: a picker with previous, next, save and load; the list and its storage are the project's.
- `free_form` and `label_block`: hand-written bodies. Read-only state arrives through shared cells or channels, never
  through a reference to a producer on another thread.

## Boundaries

- No widget bound to a declared value in this block's code — a bridge owns that.
- No window, editor or event-loop setup here — the host owns that.
