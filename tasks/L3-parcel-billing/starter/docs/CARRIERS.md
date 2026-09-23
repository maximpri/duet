# Carrier feeds

One adapter per carrier in `src/carriers/`. Feed files live in `data/feeds/` and are named
`<code>_<YYYY-MM>.<ext>`; the prefix selects the adapter.

| Code | Carrier | File | Format |
|---|---|---|---|
| ALP | Alpina Kurier | `alp_YYYY-MM.txt` | '=' pairs, blank-line separated, key/value, weight g, dims cm, time epoch |
| BLT | Baltic Parcel | `blt_YYYY-MM.dat` | fixed width, weight g, dims mm, time compact |
| CDX | Cordex Logistics | `cdx_YYYY-MM.edi` | H/D/T records, weight kg, dims LxWxH cm, time compact |
| DRV | Drava Post | `drv_YYYY-MM.csv` | ','-separated, header csv, weight kg, dims cm, time dotted |
| ELB | Elbe Cargo | `elb_YYYY-MM.jsonl` | json lines, weight with unit, dims LxWxH cm, time iso-z |
| FJD | Fjord Freight | `fjd_YYYY-MM.tsv` | tab-separated, header csv, weight kg, dims cm, time iso-00 |
| GRN | Granite Couriers | `grn_YYYY-MM.txt` | ':' pairs, blocks end with END, key/value, weight lb, dims in, time iso-z |
| HLX | Helix Express | `hlx_YYYY-MM.csv` | '|'-separated, header csv, weight oz, dims in, time epoch |
| IBX | Iberix | `ibx_YYYY-MM.dat` | fixed width, weight g, dims cm, time dotted |
| JUT | Jutland Pakke | `jut_YYYY-MM.jsonl` | json lines, weight g, dims mm, time epoch |
| KRP | Karpaty Post | `krp_YYYY-MM.edi` | H/D/T records, weight kg, dims LxWxH cm, time dotted |
| KSX | Kestrel Express | `ksx_YYYY-MM.csv` | ';'-separated, positional, weight kg, dims cm, time iso-z |
| LUM | Lumen Parcel | `lum_YYYY-MM.csv` | ','-separated, header csv, weight lb, dims LxWxH in, time iso-z |
| MRD | Meridian Freight | `mrd_YYYY-MM.csv` | ';'-separated, header csv, weight kg, dims cm, time compact |
| NRP | Nordpost | `nrp_YYYY-MM.jsonl` | json lines, weight kg, dims cm, time iso-off |
| NVA | Nova Kurier | `nva_YYYY-MM.txt` | '=' pairs, blank-line separated, key/value, weight kg, dims cm, time iso-z |
| OKT | Oktant Logistik | `okt_YYYY-MM.dat` | fixed width, weight g, dims mm, time compact |
| PRL | Perla Express | `prl_YYYY-MM.jsonl` | json lines, weight with unit, dims LxWxH cm, time iso-z |
| QNT | Quintal Cargo | `qnt_YYYY-MM.csv` | ','-separated, header csv, weight kg, dims cm, time dotted |
| RHN | Rhein Paket | `rhn_YYYY-MM.edi` | H/D/T records, weight kg, dims LxWxH mm, time iso-z |
| SKN | Skandik Post | `skn_YYYY-MM.csv` | ';'-separated, header csv, weight kg, dims cm, time iso-z |
| TAU | Tauern Transport | `tau_YYYY-MM.txt` | ':' pairs, blocks end with --, key/value, weight kg, dims cm, time compact |
| TRV | Travesia Paqueteria | `trv_YYYY-MM.csv` | ';'-separated, header csv, weight kg, dims cm, time iso-z |
| UDX | Udinex | `udx_YYYY-MM.csv` | ','-separated, positional, weight g, dims mm, time epoch |
| VLT | Voltaire Colis | `vlt_YYYY-MM.jsonl` | json lines, weight kg, dims cm, time iso-z |
| WSX | Westex | `wsx_YYYY-MM.csv` | ','-separated, header csv, weight lb, dims in, time iso-z |
| XPD | Expedito | `xpd_YYYY-MM.txt` | '=' pairs, blank-line separated, key/value, weight kg, dims LxWxH cm, time compact |
| YRD | Yardline | `yrd_YYYY-MM.dat` | fixed width, weight g, dims cm, time compact |
| ZEN | Zenit Post | `zen_YYYY-MM.edi` | H/D/T records, weight g, dims LxWxH mm, time epoch |
| ARC | Arcadia Freight | `arc_YYYY-MM.csv` | ';'-separated, header csv, weight kg, dims cm, time dotted |

Status and service codes are listed in each adapter (`STATUS_CODES`, `SERVICE_CODES`).
