"""Golden cases for beam_base::ai_registry, from the registry half of ai_describer.py."""

from __future__ import annotations

import json

from _common import outcome, write_fixture

import ai_describer as ad


def main():
    providers = []
    for pid in ["gemini", "openai", "", "nonexistent", "OpenAI", None]:
        info = ad.provider_info(pid)
        providers.append({
            "pid": pid,
            "display": ad.provider_display_name(pid),
            "models": [list(m) for m in ad.vision_models_for(pid)],
            "model_names": ad.model_names_for(pid),
            "default_model": ad.default_model_for(pid),
            "config_keys": list(ad.config_keys_for(pid)),
            "key_help": info["key_help"],
            "extras": [
                {**{k: v for k, v in ex.items() if k != "values"},
                 "values": [list(x) for x in ex.get("values", [])]}
                for ex in ad.extras_for(pid)
            ],
        })
    cfgs = [
        None, {}, {"ai_describer_openai_base_url": "  https://x/v1/ ",
                   "ai_describer_openai_reasoning_effort": "high",
                   "ai_describer_openai_detail": "low"},
        {"ai_describer_openai_base_url": "   ", "ai_describer_openai_reasoning_effort": "HIGH",
         "ai_describer_openai_detail": None},
        {"ai_describer_openai_base_url": None, "ai_describer_openai_reasoning_effort": 3},
        {"ai_describer_openai_base_url": 0},
        {"ai_describer_openai_base_url": 5},
        {"ai_describer_openai_base_url": ["x"]},
    ]
    extra_args = []
    for pid in ["openai", "gemini", "bogus"]:
        for c in cfgs:
            res = outcome(ad.extra_args_for, pid, c)
            if "ok" in res:
                res["ok"] = [[k, v] for k, v in res["ok"].items()]
            extra_args.append({"pid": pid, "cfg": json.dumps(c), **res})
    names = []
    for pid in ["gemini", "openai", "x"]:
        for m in ["gpt-4o", "models/gemini-2.5-pro", "o3", "", "unknown"]:
            names.append({"pid": pid, "model": m, "display": ad.display_name_for(m, pid)})
    write_fixture("ai_registry", {
        "constants": {
            "PROVIDERS": [list(p) for p in ad.PROVIDERS],
            "DEFAULT_PROVIDER": ad.DEFAULT_PROVIDER,
            "PROVIDER_CFG_KEY": ad.PROVIDER_CFG_KEY,
            "GEMINI_BASE": ad.GEMINI_BASE,
            "GEMINI_DEFAULT_MODEL": ad.GEMINI_DEFAULT_MODEL,
            "OPENAI_BASE": ad.OPENAI_BASE,
            "OPENAI_DEFAULT_BASE_URL": ad.OPENAI_DEFAULT_BASE_URL,
            "OPENAI_DEFAULT_MODEL": ad.OPENAI_DEFAULT_MODEL,
            "OPENAI_REASONING_EFFORTS": [list(x) for x in ad.OPENAI_REASONING_EFFORTS],
            "OPENAI_DETAIL_LEVELS": [list(x) for x in ad.OPENAI_DETAIL_LEVELS],
        },
        "all_config_keys": list(ad.all_config_keys()),
        "providers": providers,
        "extra_args": extra_args,
        "display_names": names,
    })


if __name__ == "__main__":
    main()
