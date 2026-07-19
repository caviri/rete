# farmacos-es — CIMA (AEMPS) harvest

All medicines authorized in Spain, from the official public CIMA REST API
(https://cima.aemps.es/) plus the Nomenclátor de Prescripción full dump.
No scraping: everything comes from documented JSON endpoints.

## Sources

| What | Endpoint |
|---|---|
| Medicine list (25,485) | `GET /cima/rest/medicamentos?pagina=N` (200/page) |
| Per-medicine detail | `GET /cima/rest/medicamento?nregistro=X` (ATC, active substances, presentations, doc links) |
| Prospecto / ficha técnica HTML | `docs[].urlHtml` from the detail JSON |
| Segmented documents | `GET /cima/rest/docSegmentado/contenido/{1\|2}?nregistro=X` (`Accept: application/json`) — full document split into titled sections |
| Safety notes | `GET /cima/rest/notas?nregistro=X` (when `notas: true`) |
| Informative materials | `GET /cima/rest/materiales?nregistro=X` (when `materialesInf: true`) |
| Presentations (CN level, 67,267) | `GET /cima/rest/presentaciones?pagina=N` |
| Supply problems | `GET /cima/rest/psuministro?pagina=N` |
| Nomenclátor + dictionaries | `https://listadomedicamentos.aemps.gob.es/prescripcion.zip` (XML: ATC, labs, active substances, excipients, DCP/DCPF/DCSA, envases, formas farmacéuticas) |

PDF variants of ficha técnica/prospecto and photos are NOT downloaded; their URLs
are inside each `medicamentos/detalle/<nregistro>.json`.

## Run

```
python scripts/farmacos-es/harvest.py              # full, resumable (skips existing files)
python scripts/farmacos-es/harvest.py --limit 10   # smoke test
python scripts/farmacos-es/harvest.py --only docs  # single phase
```

Output goes to `data/farmacos-es/raw/` (layout documented in harvest.py's docstring).
Failures after retries land in `raw/_errors.jsonl`; re-running retries only what's missing.
~115k requests total, 8 threads → expect a few hours.
