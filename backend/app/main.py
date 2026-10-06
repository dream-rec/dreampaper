from __future__ import annotations

from pathlib import Path

from fastapi import BackgroundTasks, FastAPI, HTTPException, UploadFile
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import FileResponse

from .adapters import normalize_base_url
from .assets import AssetStore
from .config import ConfigStore, search_needs_key
from .jobs import JobManager
from .models import AppConfig, JobCreateRequest, TestModelRequest
from .prompts import PromptStore
from .templates import TemplateStore


ROOT = Path(__file__).resolve().parents[2]
BENCH_ROOT = ROOT / "PaperBananaBench"
PROMPT_ROOT = ROOT / "prompts"

config_store = ConfigStore()
asset_store = AssetStore()
template_store = TemplateStore(BENCH_ROOT)
prompt_store = PromptStore(PROMPT_ROOT)
job_manager = JobManager(template_store, asset_store, prompt_store, config_store)

app = FastAPI(title="dreampaper", version="0.1.0")
app.add_middleware(
    CORSMiddleware,
    allow_origins=["http://localhost:5173", "http://127.0.0.1:5173"],
    allow_credentials=True,
    allow_methods=["*"],
    allow_headers=["*"],
)


@app.get("/api/health")
def health() -> dict[str, str]:
    return {"status": "ok", "name": "dreampaper"}


@app.get("/api/config/models")
def get_model_config():
    return config_store.public()


@app.put("/api/config/models")
def save_model_config(config: AppConfig):
    saved = config_store.save(config)
    return config_store.public()


@app.post("/api/config/models/test")
def test_model_config(request: TestModelRequest) -> dict[str, str]:
    profile = request.profile
    normalized = normalize_base_url(profile.base_url, profile.protocol)
    # 免密钥的搜索协议（duckduckgo）不看密钥，只报地址。
    if not profile.api_key and search_needs_key(profile.protocol):
        return {"status": "missing_api_key", "base_url": normalized}
    return {"status": "configured", "base_url": normalized}


@app.get("/api/templates")
def list_templates(kind: str | None = None, q: str | None = None, limit: int = 80):
    return template_store.list(kind=kind, q=q, limit=limit)


@app.get("/api/templates/{template_id}/image")
def template_image(template_id: str):
    return template_store.image_response(template_id)


@app.post("/api/assets")
async def upload_asset(file: UploadFile):
    return await asset_store.save_upload(file)


@app.get("/api/assets/{asset_id}")
def get_asset(asset_id: str):
    return asset_store.response(asset_id)


@app.post("/api/jobs")
async def create_job(request: JobCreateRequest, background_tasks: BackgroundTasks):
    record = job_manager.create(request)
    background_tasks.add_task(job_manager.run, record.id, request)
    return record


@app.get("/api/jobs/{job_id}")
def get_job(job_id: str):
    try:
        return job_manager.get(job_id)
    except KeyError as exc:
        raise HTTPException(status_code=404, detail="Job not found") from exc


@app.get("/api/jobs/{job_id}/images/{name}")
def get_job_image(job_id: str, name: str):
    try:
        return FileResponse(job_manager.image_path(job_id, name))
    except FileNotFoundError as exc:
        raise HTTPException(status_code=404, detail="Image not found") from exc

