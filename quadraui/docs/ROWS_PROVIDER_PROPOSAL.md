# `Rows<T>` — virtualised row storage for the collection descriptors

> **DESIGN SKETCH. Nothing here is implemented, and the decision is to DEFER
> (2026-09-07, issue #837).**
>
> No descriptor changes. This document exists so the limit below is written
> down with its cost, and so whoever hits it does not re-derive the analysis.
> The trigger for building it is stated in §7.

**Date:** 2026-09-07. Issue: **#837**. Parent epic: **#788**.

---

## 1. The limit, measured

Five descriptors own their full row collection by value:

| Descriptor | Field | Location |
|---|---|---|
| `DataTable` | `pub rows: Vec<DataRow>` | `primitives/data_table.rs:146` |
| `ListView` | `pub items: Vec<ListItem>` | `primitives/list.rs:31` |
| `TreeView` | `pub rows: Vec<TreeRow>` | `primitives/tree.rs:42` |
| `TextDisplay` | `pub lines: Vec<TextDisplayLine>` | `primitives/text_display.rs:54` |
| `Editor` | `pub lines: Vec<EditorLine>` | `primitives/editor.rs:305` |

Primitives are declarative and the host rebuilds them from its own state each
frame — `CONSUMER_PATTERNS.md` states this directly ("the host rebuilds a fresh
`MultiSectionView` from its state every frame"). Combined with ownership by
value, a million-row table means allocating and populating a million-element
`Vec` per frame, of which the backend paints the visible window — on the order
of 50 rows.

No app has hit this. vimcode's largest real collections are file trees and
diagnostic lists in the low thousands, where the per-frame rebuild is not
measurable against the paint itself.

## 2. The sketch

```rust
pub enum Rows<T> {
    Owned(Vec<T>),
    Provider { len: usize, get: Box<dyn Fn(Range<usize>) -> &[T]> },
}
```

Each descriptor's `Vec<T>` field becomes `Rows<T>`. A host with a small
collection keeps passing `Owned` and nothing changes for it. A host with a
large one supplies `len` plus a windowed accessor, and only the visible window
is ever materialised.

## 3. Hit-testing — unaffected

This is the part that costs nothing, and it is worth stating because it looks
like it should be the hard part.

`hit_test` is a method on the **Layout**, not on the descriptor, and it returns
an index:

- `ListViewLayout::hit_test(x, y) -> ListViewHit`, whose `Item(usize)` carries
  "the item's index into `ListView.items`" (`primitives/list.rs:154-163`).
- `DataTableLayout::hit_test(...) -> DataTableHit`, with `Row { idx: usize }`
  (`primitives/data_table.rs:230-242`).

`DataTableLayout` holds `row_height`, `visible_rows`, `columns`,
`viewport_height`, `scrollbar_width` — resolved geometry, and no row data
(`primitives/data_table.rs:254-266`). Hit-testing is therefore already
arithmetic over `len` and the scroll offset. It never dereferences a row, so it
does not care how rows are stored, and the app resolving an index back to its
own data is already the established pattern.

The same holds for the `*_layout` trait methods: `data_table_layout(&self,
rect, table) -> DataTableLayout` (`backend.rs:1297`), `list_layout`
(`backend.rs:1330`), `tree_layout` (`backend.rs:1929`). Each needs `len` and
per-row height, not the rows.

## 4. Where it works

**`DataTable`, `ListView`, `TextDisplay`.** Flat, index-addressable, uniform
row height. Layout needs `len`; paint needs the visible window; hit-testing
needs neither. These three are a clean fit.

## 5. Where it does not work

**`Editor` needs a full scan for horizontal extent.** The horizontal scrollbar
extent is the width of the longest line, which cannot be answered from a
window. A `Provider` would have to either expose a `max_line_width` alongside
`len` — pushing a layout concern into the host — or force a full scan and give
up the point of the exercise.

**`TreeView` rows are a flattened projection, not a range.** `TreeRow` carries
depth and expansion state, so index *i* depends on which ancestors are
expanded. A provider over a tree has to serve the flattened, currently-expanded
row set, which means it must track expansion — state the descriptor currently
holds. This is a different design, not this one.

**The descriptor derive invariant is the blocker, and it is fatal to the sketch
as written.** All five descriptors derive their equality and serialisation:

```
DataTable    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
ListView     #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
TreeView     #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
TextDisplay  #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
Editor       #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
```

A `Box<dyn Fn>` satisfies none of `Debug`, `Clone`, `PartialEq`, `Eq`,
`Serialize` or `Deserialize`. Putting a closure in a descriptor breaks all six
on every descriptor that adopts it.

That is not a derive inconvenience. `PartialEq` on descriptors is what lets a
host diff last frame's descriptor against this frame's to decide whether to
repaint, and `Serialize`/`Deserialize` is what lets conformance fixtures
describe a screen as data. A `Rows<T>` that carries a closure would either
force those impls to be hand-written and lie about the `Provider` arm — two
providers comparing equal because closures are not comparable — or remove the
capability from the descriptors that adopt it.

**The borrow signature does not hold either.** `Fn(Range<usize>) -> &[T]`
cannot return a reference to a value the closure computes, because there is
nothing for the reference to borrow from. It only works when the provider
already owns contiguous backing storage for that range — which is the case the
host could have handed over as `Owned` anyway. A provider that generates rows
(from a database page, a decompressed block, a log tail) needs
`get_into(&mut Vec<T>, Range<usize>)`, or a returned `Cow<[T]>`, or an
owned-window return. Each of those reintroduces an allocation per frame,
smaller than the full collection but no longer zero.

## 6. What would have to change first

In order, cheapest first:

1. **Settle what `PartialEq` on a descriptor means** once part of it is opaque.
   The honest options are dropping `PartialEq` from adopting descriptors, or a
   `Rows::Provider` arm that compares only `len` and a host-supplied generation
   counter. The second keeps frame-diffing working and is the only one that
   does not push work onto every consumer.
2. **Settle serialisation.** Conformance fixtures serialise descriptors. A
   `Provider` cannot round-trip, so either fixtures only ever use `Owned` (and
   the type admits that in its docs), or `Serialize` is dropped from adopting
   descriptors and the fixture format changes.
3. **Fix the accessor signature** to one that can return computed rows.
4. **Only then** the per-descriptor work, and only for the three in §4.

## 7. Decision: defer

Not built. The cost is a breaking change to six derives across five public
descriptors plus a redesign of frame-diffing and the conformance fixture
format, and the benefit today is zero because no app is near the wall.

**Build it when an app ships a collection whose per-frame rebuild is a
measured cost** — concretely, when a profile attributes a visible fraction of
frame time to constructing one of these five `Vec`s, on a real workload rather
than a synthetic one. Anything below that is cheaper to solve in the host by
handing the descriptor a pre-sliced window of its own data, which needs no
crate change at all and is what an app hitting tens of thousands of rows should
do first.

Re-read §5 before starting. The derive invariant is the reason this is not a
small change, and it is the thing most likely to be discovered late.
