# API D-Bus

> Brouillon. Les signatures seront figées en phase 4 (daemon) et 6 (clipd), avant les frontends.
> Règle : changement incompatible = nouvelle interface (`…2`).

## `org.blazorde.MsiCenter1` — bus système, `msi-daemon`

Objet : `/org/blazorde/MsiCenter`

| Membre | Type | Signature (prévue) | Polkit |
|---|---|---|---|
| `Profile` | propriété | `s` (`eco` · `balanced` · `gaming`) | lecture libre |
| `SetProfile` | méthode | `(s) → ()` | `org.blazorde.msicenter.set-profile` |
| `AutoProfileOnPower` | propriété rw | `b` | idem |
| `FanCurve` | propriété | `a{s(ddd)}` par ventilo : `min, max, exposant` | lecture libre |
| `SetFanCurve` | méthode | `(s fan, d t0, d t1, d min, d max, d k) → ()` | `…set-fan-curve` |
| `CoolerBoost` | propriété rw | `b` | `…set-fan-curve` |
| `ChargeLimit` | propriété rw | `y` (50–100) | `…set-battery` |
| `GpuState` | propriété | `s` (`active` · `suspended` · `removed`) | lecture libre |
| `GpuHolders` | méthode | `() → a(us)` (pid, nom) | lecture libre |
| `Telemetry` | signal | `a{sv}` (cpu_pct, dgpu_pct, igpu_pct, ram_pct, npu_pct, cpu_temp, gpu_temp, fan1_rpm, fan2_rpm, power_w) | – |
| `SafetyEvent` | signal | `(s kind, d value)` | – |

## `org.blazorde.Clipd1` — bus session, `clipd`

Objet : `/org/blazorde/Clipd`

| Membre | Type | Signature (prévue) |
|---|---|---|
| `Add` | méthode | `(s mime, ay data) → t id` |
| `List` | méthode | `(s query, as tags, u offset, u limit) → a(tsstbas)` (id, mime, aperçu, date, épinglé, tags) |
| `Get` | méthode | `(t id) → (s mime, ay data)` |
| `Pin` / `Unpin` | méthode | `(t id) → ()` |
| `Delete` | méthode | `(at ids) → ()` |
| `SetTags` | méthode | `(t id, as tags) → ()` |
| `Tags` | méthode | `() → a(su)` (tag, nombre) |
| `Changed` | signal | `()` |
