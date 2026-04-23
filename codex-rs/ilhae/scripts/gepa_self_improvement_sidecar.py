#!/usr/bin/env python3
from __future__ import annotations

import json
import os
import sys
from dataclasses import asdict, dataclass, field
from pathlib import Path


@dataclass
class SidecarRequest:
    kind: str
    preset: str
    subject: str
    detail: str
    prompt: str
    instructions: str
    task_history: list[str] = field(default_factory=list)
    top_paths: list[str] = field(default_factory=list)
    group_count: int | None = None


@dataclass
class SidecarResponse:
    optimized_prompt: str
    optimized_instructions: str
    optimization_status: str
    optimizer: str
    reason: str | None = None
    score: float | None = None


def repo_root() -> Path:
    return Path(__file__).resolve().parents[4]


def load_request() -> SidecarRequest:
    payload = json.load(sys.stdin)
    return SidecarRequest(
        kind=str(payload.get("kind", "")).strip(),
        preset=str(payload.get("preset", "")).strip(),
        subject=str(payload.get("subject", "")).strip(),
        detail=str(payload.get("detail", "")).strip(),
        prompt=str(payload.get("prompt", "")).strip(),
        instructions=str(payload.get("instructions", "")).strip(),
        task_history=[
            str(item).strip()
            for item in payload.get("task_history", []) or []
            if str(item).strip()
        ],
        top_paths=[
            str(item).strip()
            for item in payload.get("top_paths", []) or []
            if str(item).strip()
        ],
        group_count=(
            int(payload["group_count"])
            if payload.get("group_count") is not None
            else None
        ),
    )


def normalize_text(text: str) -> str:
    lines = [line.strip() for line in text.splitlines() if line.strip()]
    return "\n".join(lines).strip()


def build_rule_candidate(request: SidecarRequest) -> tuple[str, str]:
    detail_lower = request.detail.lower()
    prompt_parts = [
        request.prompt.rstrip("."),
        "Prefer low-risk summarize decisions first, escalate ambiguous groups instead of forcing apply actions, and leave an audit-ready decision trail.",
        "When evidence is thin, convert the result into a concrete review task instead of mutating memory aggressively.",
    ]
    if "ignored" in detail_lower:
        prompt_parts.append(
            "Treat ignored groups as review candidates that need an explicit keep-ignored or requeue decision."
        )
    if "pending" in detail_lower or "stale" in detail_lower:
        prompt_parts.append(
            "Prioritize the oldest or most repetitive pending groups so the queue shrinks without widening risk."
        )
    if request.group_count and request.group_count > 0:
        prompt_parts.append(
            f"Shrink the current review queue of {request.group_count} groups without broadening memory mutation risk."
        )

    instruction_lines = [
        request.instructions.rstrip("."),
        "Classify each dream group into summarize, promote, extract, ignore, or defer before taking action.",
        "Only promote or extract when the output is stable, reusable across sessions, and clearly scoped.",
        "If the reusable output is procedural rather than factual knowledge, inspect existing skills first and use skill_upsert only for a concise SKILL.md under brain/skills/custom.",
        "If a group is ambiguous, summarize it or leave a follow-up note instead of auto-applying a risky change.",
        "Record which evidence justified the decision so the next reviewer can audit the result quickly.",
    ]
    if "ignored" in detail_lower:
        instruction_lines.append(
            "For ignored groups, compare requeue value against repeat-risk and keep them ignored unless the benefit is obvious."
        )
    if request.top_paths:
        instruction_lines.append(
            "Use the current top paths as review anchors before generalizing decisions: "
            + ", ".join(request.top_paths[:3])
            + "."
        )

    return normalize_text(" ".join(prompt_parts)), normalize_text("\n".join(instruction_lines))


def score_candidate(
    request: SidecarRequest, prompt: str, instructions: str
) -> tuple[float, list[str]]:
    prompt_lower = prompt.lower()
    instructions_lower = instructions.lower()
    score = 0.0
    feedback: list[str] = []

    if "low-risk" in prompt_lower:
        score += 0.2
    else:
        feedback.append("Make the prompt explicitly favor low-risk summarize-first decisions.")

    if "audit" in prompt_lower or "trail" in prompt_lower:
        score += 0.15
    else:
        feedback.append("Require an audit-ready explanation in the prompt.")

    if "review" in prompt_lower or "summarize" in prompt_lower:
        score += 0.15
    else:
        feedback.append("Keep the prompt grounded in review/summarize actions.")

    if "classify each dream group" in instructions_lower:
        score += 0.15
    else:
        feedback.append("Start instructions with an explicit classify-first step.")

    if "stable" in instructions_lower and "reusable" in instructions_lower:
        score += 0.15
    else:
        feedback.append("Gate promote/extract on stable, reusable outputs.")

    if "ambiguous" in instructions_lower:
        score += 0.1
    else:
        feedback.append("Tell the agent to defer or summarize ambiguous groups instead of forcing apply.")

    if "memory_dream_" in instructions_lower:
        score += 0.1
    else:
        feedback.append("Keep instructions inside memory_dream tool scope.")

    if "skill_upsert" in instructions_lower and "brain/skills/custom" in instructions_lower:
        score += 0.05
    else:
        feedback.append("Preserve skill_upsert guidance for procedural learning.")

    if request.top_paths:
        score += 0.05

    return min(score, 1.0), feedback


def maybe_add_local_dspy() -> None:
    local_dspy = repo_root() / "dspy"
    if local_dspy.exists():
        sys.path.insert(0, str(local_dspy))


def run_live_dspy_gepa(
    request: SidecarRequest,
    candidate_prompt: str,
    candidate_instructions: str,
) -> SidecarResponse | None:
    task_model = os.getenv("ILHAE_GEPA_TASK_MODEL", "").strip()
    if not task_model:
        return None

    maybe_add_local_dspy()

    import dspy  # type: ignore

    reflection_model = os.getenv("ILHAE_GEPA_REFLECTION_MODEL", "").strip() or task_model
    max_metric_calls = int(os.getenv("ILHAE_GEPA_MAX_METRIC_CALLS", "6"))
    task_temperature = float(os.getenv("ILHAE_GEPA_TASK_TEMPERATURE", "0.2"))
    reflection_temperature = float(
        os.getenv("ILHAE_GEPA_REFLECTION_TEMPERATURE", "1.0")
    )

    class FollowupProgram(dspy.Module):
        def __init__(self, base_prompt: str, base_instructions: str) -> None:
            super().__init__()
            self.prompt_builder = dspy.Predict("subject, detail -> prompt")
            self.prompt_builder.signature.instructions = base_prompt
            self.instructions_builder = dspy.Predict("subject, detail -> instructions")
            self.instructions_builder.signature.instructions = base_instructions

        def forward(self, subject: str, detail: str):
            prompt = self.prompt_builder(subject=subject, detail=detail).prompt
            instructions = self.instructions_builder(
                subject=subject, detail=detail
            ).instructions
            return dspy.Prediction(prompt=prompt, instructions=instructions)

    def metric(example, pred, trace=None, pred_name=None, pred_trace=None):
        prompt = normalize_text(str(getattr(pred, "prompt", "")))
        instructions = normalize_text(str(getattr(pred, "instructions", "")))
        score, feedback = score_candidate(request, prompt, instructions)
        return dspy.Prediction(score=score, feedback=" ".join(feedback))

    task_lm = dspy.LM(task_model, cache=False, temperature=task_temperature)
    reflection_lm = dspy.LM(
        reflection_model,
        cache=False,
        temperature=reflection_temperature,
    )
    student = FollowupProgram(request.prompt, request.instructions)
    trainset = [
        dspy.Example(
            subject=request.subject,
            detail=request.detail,
            prompt=candidate_prompt,
            instructions=candidate_instructions,
        ).with_inputs("subject", "detail")
    ]

    with dspy.context(lm=task_lm):
        optimizer = dspy.GEPA(
            metric=metric,
            reflection_lm=reflection_lm,
            max_metric_calls=max_metric_calls,
            component_selector="all",
        )
        optimized_program = optimizer.compile(student, trainset=trainset, valset=trainset)
        prediction = optimized_program(subject=request.subject, detail=request.detail)

    optimized_prompt = normalize_text(
        str(getattr(prediction, "prompt", "")).strip() or candidate_prompt
    )
    optimized_instructions = normalize_text(
        str(getattr(prediction, "instructions", "")).strip() or candidate_instructions
    )
    score, _ = score_candidate(request, optimized_prompt, optimized_instructions)
    return SidecarResponse(
        optimized_prompt=optimized_prompt,
        optimized_instructions=optimized_instructions,
        optimization_status="optimized",
        optimizer="dspy_gepa",
        reason=f"task_model={task_model}",
        score=score,
    )


def main() -> int:
    request = load_request()
    candidate_prompt, candidate_instructions = build_rule_candidate(request)

    if request.kind not in {
        "self_improvement_followup",
        "self_improvement_followup_offline",
    }:
        response = SidecarResponse(
            optimized_prompt=request.prompt,
            optimized_instructions=request.instructions,
            optimization_status="fallback",
            optimizer="passthrough",
            reason=f"unsupported kind: {request.kind}",
            score=None,
        )
        print(json.dumps(asdict(response)))
        return 0

    try:
        live_response = run_live_dspy_gepa(
            request, candidate_prompt, candidate_instructions
        )
        if live_response is not None:
            print(json.dumps(asdict(live_response)))
            return 0
    except Exception as exc:  # pragma: no cover - defensive runtime fallback
        score, _ = score_candidate(request, candidate_prompt, candidate_instructions)
        response = SidecarResponse(
            optimized_prompt=candidate_prompt,
            optimized_instructions=candidate_instructions,
            optimization_status="fallback",
            optimizer="fallback_rules",
            reason=f"live GEPA unavailable: {type(exc).__name__}: {exc}",
            score=score,
        )
        print(json.dumps(asdict(response)))
        return 0

    score, _ = score_candidate(request, candidate_prompt, candidate_instructions)
    response = SidecarResponse(
        optimized_prompt=candidate_prompt,
        optimized_instructions=candidate_instructions,
        optimization_status="fallback",
        optimizer="fallback_rules",
        reason="set ILHAE_GEPA_TASK_MODEL to enable live dspy.GEPA optimization",
        score=score,
    )
    print(json.dumps(asdict(response)))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
