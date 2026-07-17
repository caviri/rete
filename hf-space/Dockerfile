# rete graph gateway — Docker SDK. Three planes on port 7860:
#   /data  HTTP Range + CORS byte serving (the original gateway)
#   /api   SPARQL query plane over the published .rete catalog (FastAPI)
#   /mcp   MCP server (FastMCP, streamable HTTP) for LLM apps
FROM python:3.11-slim

WORKDIR /app
COPY requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt
COPY app.py rete_service.py rete_api.py rete_mcp.py rete_ask.py catalog.json branding.json ./

# HF persistent storage (and any mounted bucket) lives at /data; the disk
# range-cache defaults to /data/.rete-cache so it survives restarts.
ENV DATA_DIR=/data \
    WEB_CONCURRENCY=1 \
    THREADPOOL_TOKENS=200
EXPOSE 7860

# One worker by default: the engine releases the GIL (queries run in the
# threadpool with real parallelism) and resident graph handles are not cheap
# to duplicate per process. Raise WEB_CONCURRENCY only with RAM to spare.
CMD uvicorn app:app --host 0.0.0.0 --port 7860 --workers ${WEB_CONCURRENCY:-1} --timeout-keep-alive 75
