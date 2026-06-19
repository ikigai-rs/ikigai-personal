# ikigai-personal

Personal contexts for [ikigai](https://github.com/ikigai-rs) — OS-backed
resources under `urn:personal:*`, resolved through the local operating system so
the data never leaves the machine.

| resource | what it is |
|----------|------------|
| `urn:personal:contacts` | the local address book |
| `urn:personal:calendar` | the detailed calendar (titles, times, attendees) |
| `urn:personal:availability` | the **free/busy projection** — busy blocks only, no detail |

## Platform support

| platform | status |
|----------|--------|
| **macOS** | implemented (currently **sample data**, pending real Contacts/EventKit) |
| **Windows** | placeholder — builds, resolves to a clear "not supported yet" error |
| **Linux** | placeholder — builds, resolves to a clear "not supported yet" error |

Adding a backend means filling in one file under `src/platform/` — the seam and
the rest of the crate don't change.

## Detail vs. availability

`calendar` is the detailed view; `availability` is the free/busy projection. Today
they are two resources. Once capability **attenuation** lands they collapse into
one resource with two capability-scoped projections — an agent granted only the
free/busy capability sees availability and never the detail. That's the
data-minimization story: *"an agent books around my week without ever learning
what I'm doing."*

## Usage

```rust
use ikigai_core::Kernel;
use std::sync::Arc;

let kernel = Kernel::new(Arc::new(ikigai_personal::space()));
```

Personal data is treated as a live fact and is **uncacheable**.

## License

Licensed under either of [Apache-2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT) at
your option.
