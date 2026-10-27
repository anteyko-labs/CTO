#!/usr/bin/env python3
"""Реестр-навигатор проекта: docs/nav/registry.json.

Собирает из исходников краткую карту со ссылками «файл:строка»: SPEC, ADR, маршруты сервера
и клиента, файлы сервера и клиента с функциями, миграции и таблицы, тесты. Без тел функций.

Запуск из корня репозитория (или любой папки — путь берётся от файла скрипта):
    python3 scripts/registry.py           # пересобрать docs/nav/registry.json
    python3 scripts/registry.py --check   # код 1, если файл устарел

Вывод детерминирован: всё отсортировано, меток времени нет — повторный запуск файл не меняет.
Только стандартная библиотека Python 3.

Список индексируемых путей повторён в .githooks/pre-commit (п. 5): новый источник — правка и там.
"""

import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "docs" / "nav" / "registry.json"

ADR_RE = re.compile(r"ADR-(\d{3})")
SPEC_RE = re.compile(r"SPEC-(\d{2})")
CODE_PATH_RE = re.compile(r"(?<![\w/.-])((?:[\w-]+/)*[\w.-]*\w\.(?:rs|tsx|ts|sql|ps1|yml|sh|py))\b")


def allowed_files() -> set[str]:
    """Файлы из .githooks/forbidden-allow (ADR-060): их свободный текст в реестр не переносится —
    реестр проверяется хуком как обычный файл."""
    p = ROOT / ".githooks" / "forbidden-allow"
    if not p.exists():
        return set()
    return {ln.strip() for ln in p.read_text(encoding="utf-8").splitlines() if ln.strip() and not ln.startswith("#")}


ALLOW = allowed_files()


def rel(p: Path) -> str:
    return p.relative_to(ROOT).as_posix()


def read_lines(p: Path) -> list[str]:
    return p.read_text(encoding="utf-8").splitlines()


def line_of(text: str, offset: int) -> int:
    return text.count("\n", 0, offset) + 1


def ids(regex: re.Pattern, text: str, prefix: str) -> list[str]:
    return sorted({f"{prefix}-{m}" for m in regex.findall(text)})


def short(s: str, n: int = 120) -> str:
    s = re.sub(r"\s+", " ", s).strip()
    return s if len(s) <= n else s[: n - 1].rstrip() + "…"


def files(base: str, pattern: str) -> list[Path]:
    d = ROOT / base
    return sorted(d.glob(pattern), key=lambda p: rel(p)) if d.exists() else []


# ---------- документы ----------


def frontmatter(lines: list[str]) -> dict:
    out: dict = {}
    if lines and lines[0].strip() == "---":
        for ln in lines[1:]:
            if ln.strip() == "---":
                break
            if ":" in ln:
                k, v = ln.split(":", 1)
                out[k.strip()] = v.strip()
    return out


def specs() -> list[dict]:
    out = []
    for p in files("docs/tier-4", "spec-*.md"):
        lines = read_lines(p)
        text = "\n".join(lines)
        fm = frontmatter(lines)
        num = re.match(r"spec-(\d+)", p.name)
        title = ""
        sections = []
        for i, ln in enumerate(lines, 1):
            if not title and ln.startswith("# "):
                title = re.sub(r"^SPEC-\d+\.\s*", "", ln[2:].strip())
            elif ln.startswith("## ") or ln.startswith("### "):
                level = 2 if ln.startswith("## ") else 3
                sections.append({"title": short(ln.lstrip("#").strip(), 100), "line": i, "level": level})
        out.append(
            {
                "id": f"SPEC-{num.group(1) if num else '??'}",
                "file": rel(p),
                "lines": len(lines),
                "title": title,
                "status": fm.get("status", ""),
                "updated": fm.get("updated", ""),
                "sections": sections,
                "adr": ids(ADR_RE, text, "ADR"),
                "spec": [s for s in ids(SPEC_RE, text, "SPEC") if s != f"SPEC-{num.group(1) if num else ''}"],
                "code": sorted(set(CODE_PATH_RE.findall(text))),
            }
        )
    return sorted(out, key=lambda s: s["id"])


def adrs() -> list[dict]:
    p = ROOT / "docs" / "tier-5" / "decisions.md"
    if not p.exists():
        return []
    lines = read_lines(p)
    heads = [(i, ln) for i, ln in enumerate(lines, 1) if re.match(r"^## ADR-\d{3}", ln)]
    out = []
    for k, (start, head) in enumerate(heads):
        end = heads[k + 1][0] - 1 if k + 1 < len(heads) else len(lines)
        while end > start and not lines[end - 1].strip():
            end -= 1
        body = lines[start - 1 : end]
        m = re.match(r"^## (ADR-\d{3})\.?\s*(.*)$", head)
        adr_id, title = m.group(1), m.group(2).strip()
        text = "\n".join(body)
        status = ""
        status_line = None
        date = ""
        for j, ln in enumerate(body):
            if not status and ln.startswith("- **Статус:**"):
                status = ln.split("**Статус:**", 1)[1].strip()
                status_line = start + j
            if not date and ln.startswith("- **Дата:**"):
                date = ln.split("**Дата:**", 1)[1].strip()
        relations = {}
        low = status.lower()
        for word, key in (
            ("заменено", "replaced_by"),
            ("отменено", "cancelled_by"),
            ("уточнено", "refined_by"),
            ("уточняет", "refines"),
            ("дополняет", "extends"),
            ("заменяет", "replaces"),
        ):
            if word in low:
                refs = [r for r in ids(ADR_RE, status, "ADR") + ids(SPEC_RE, status, "SPEC") if r != adr_id]
                relations[key] = refs
        out.append(
            {
                "id": adr_id,
                "title": title,
                "date": date,
                "start": start,
                "end": end,
                "status": short(status) if status else "",
                "status_line": status_line,
                "relations": relations,
                "adr": [a for a in ids(ADR_RE, text, "ADR") if a != adr_id],
                "spec": ids(SPEC_RE, text, "SPEC"),
            }
        )
    # Обратные ссылки: в каком ADR сказано, что этот заменён/уточнён (ссылка из статуса другого).
    by_id = {a["id"]: a for a in out}
    for a in out:
        for key in ("refines", "extends", "replaces"):
            for target in a["relations"].get(key, []):
                if target in by_id:
                    by_id[target].setdefault("touched_by", [])
                    if a["id"] not in by_id[target]["touched_by"]:
                        by_id[target]["touched_by"].append(a["id"])
    for a in out:
        if "touched_by" in a:
            a["touched_by"].sort()
    return out


# ---------- сервер ----------

FN_RE = re.compile(r"^( {0,4})(pub(?:\([\w:]+\))? )?(?:const )?(?:async )?(?:unsafe )?fn (\w+)")
ITEM_OPEN_RE = re.compile(r"^(?:pub(?:\([\w:]+\))? )?(impl\b|trait\b|mod\b|struct\b|enum\b|fn\b|async fn\b)")


def rust_fns(lines: list[str]) -> list[dict]:
    """Функции верхнего уровня и методы в impl/trait (без вложенных модулей вроде tests)."""
    out = []
    block = None  # вид текущего блока верхнего уровня
    owner = ""  # тип impl/имя trait для методов
    for i, ln in enumerate(lines, 1):
        if ln and not ln[0].isspace():
            m = ITEM_OPEN_RE.match(ln)
            if m:
                block = m.group(1).split()[0]
                owner = ""
                im = re.match(r"^(?:pub(?:\([\w:]+\))? )?(?:impl|trait)(?:<[^>]*>)?\s+(?:[\w:<>, ]+?\s+for\s+)?([\w:]+)", ln)
                if im:
                    owner = im.group(1).split("::")[-1]
            elif ln.startswith("}"):
                pass
            elif not ln.startswith("#") and not ln.startswith("//"):
                block = None
        m = FN_RE.match(ln)
        if not m:
            continue
        indent = len(m.group(1))
        if indent == 0 or (indent == 4 and block in ("impl", "trait")):
            item = {"name": m.group(3), "line": i}
            if m.group(2):
                item["pub"] = True
            if indent == 4:
                item["method"] = True
                if owner:
                    item["in"] = owner
            out.append(item)
    return out


def module_doc(lines: list[str]) -> str:
    for ln in lines:
        s = ln.strip()
        if s.startswith("//!"):
            t = s[3:].strip()
            if t:
                return short(t, 160)
        elif s and not s.startswith("#!"):
            break
    return ""


def server_files() -> list[dict]:
    out = []
    for p in files("server/src", "**/*.rs"):
        lines = read_lines(p)
        about = "" if rel(p) in ALLOW else module_doc(lines)
        out.append({"file": rel(p), "lines": len(lines), "about": about, "fns": rust_fns(lines)})
    return out


ROUTE_RE = re.compile(r"\.route\(\s*\"([^\"]+)\"\s*,")
HTTP_METHODS = ("get", "post", "put", "patch", "delete", "head", "options")


def call_args(text: str, start: int) -> str:
    """Текст от start до закрывающей скобки вызова (скобки считаются, строки не разбираются)."""
    depth = 1
    for i in range(start, len(text)):
        c = text[i]
        if c == "(":
            depth += 1
        elif c == ")":
            depth -= 1
            if depth == 0:
                return text[start:i]
    return text[start:]


METHOD_RE = re.compile(r"(?:routing::|\.)\s*(\w+)\(([\w:]+)\)")
PERM_RE = re.compile(r"require_permission\(\s*[^,]+,\s*[^,]+,\s*(\"[\w.-]+\"|[A-Z_][A-Z0-9_]*)\s*,?\s*\)", re.S)
CONST_RE = re.compile(r"const\s+([A-Z_][A-Z0-9_]*)\s*:\s*&(?:'static\s+)?str\s*=\s*\"([^\"]+)\"")


def api_prefix() -> str:
    lib = ROOT / "server" / "src" / "lib.rs"
    if lib.exists():
        m = re.search(r"\.nest\(\s*\"([^\"]+)\"\s*,\s*api\s*\)", lib.read_text(encoding="utf-8"))
        if m:
            return m.group(1)
    return ""


def routes() -> list[dict]:
    prefix = api_prefix()
    out = []
    for p in files("server/src", "**/*.rs"):
        text = p.read_text(encoding="utf-8")
        if ".route(" not in text:
            continue
        lines = text.splitlines()
        fns = rust_fns(lines)
        consts = dict(CONST_RE.findall(text))
        fn_line = {f["name"]: f["line"] for f in fns if not f.get("method")}
        bounds = sorted(f["line"] for f in fns if not f.get("method"))

        def body(name: str) -> str:
            start = fn_line.get(name)
            if not start:
                return ""
            nxt = [b for b in bounds if b > start]
            end = nxt[0] - 1 if nxt else len(lines)
            return "\n".join(lines[start - 1 : end])

        for m in ROUTE_RE.finditer(text):
            path = m.group(1)
            line = line_of(text, m.start())
            for meth, handler in METHOD_RE.findall(call_args(text, m.end())):
                if meth not in HTTP_METHODS:
                    continue
                local = handler.split("::")[-1]
                item = {
                    "method": meth.upper(),
                    "path": prefix + path,
                    "file": rel(p),
                    "line": line,
                    "handler": handler,
                }
                if local in fn_line and "::" not in handler:
                    item["handler_line"] = fn_line[local]
                    b = body(local)
                    # Проверка прав бывает в вспомогательной функции того же файла — смотрим на шаг глубже.
                    called = {c for c in re.findall(r"\b(\w+)\(", b) if c in fn_line and c != local}
                    b_all = b + "\n" + "\n".join(body(c) for c in sorted(called))
                    perms = []
                    for pm in PERM_RE.findall(b_all):
                        perms.append(pm.strip('"') if pm.startswith('"') else consts.get(pm, pm))
                    if perms:
                        item["permission"] = sorted(set(perms))
                    if "require_owner" in b_all:
                        item["owner_only"] = True
                out.append(item)
    return sorted(out, key=lambda r: (r["path"], r["method"], r["file"], r["line"]))


# ---------- клиент ----------

EXPORT_RE = re.compile(
    r"^export\s+(default\s+)?(?:(async\s+)?function\*?\s+(\w+)|(const|let)\s+(\w+)|class\s+(\w+)"
    r"|(type|interface|enum)\s+(\w+))"
)


def first_comment(lines: list[str]) -> str:
    for ln in lines[:5]:
        s = ln.strip()
        if s.startswith("//"):
            return short(s.lstrip("/").strip(), 160)
        if s.startswith("/*"):
            return short(s.lstrip("/*").rstrip("*/").strip(), 160)
        if s and not s.startswith("import"):
            break
    return ""


def web_files() -> list[dict]:
    out = []
    paths = [p for p in files("web/src", "**/*.ts") + files("web/src", "**/*.tsx") if ".test." not in p.name]
    for p in sorted(paths, key=rel):
        lines = read_lines(p)
        exports = []
        types = []
        for i, ln in enumerate(lines, 1):
            m = EXPORT_RE.match(ln)
            if not m:
                continue
            if m.group(3):
                exports.append({"name": m.group(3), "line": i})
            elif m.group(5):
                exports.append({"name": m.group(5), "line": i})
            elif m.group(6):
                exports.append({"name": m.group(6), "line": i})
            elif m.group(8):
                types.append({"name": m.group(8), "line": i})
        item = {"file": rel(p), "lines": len(lines), "about": first_comment(lines), "exports": exports}
        if types:
            item["types"] = types
        out.append(item)
    return out


WEB_ROUTE_RE = re.compile(r"<Route\s+path=\"([^\"]+)\"\s+element=\{(.*?)\}\s*/>")


def web_routes() -> list[dict]:
    p = ROOT / "web" / "src" / "App.tsx"
    if not p.exists():
        return []
    out = []
    for i, ln in enumerate(read_lines(p), 1):
        m = WEB_ROUTE_RE.search(ln)
        if not m:
            continue
        tags = [t for t in re.findall(r"<([A-Z]\w*)", m.group(2)) if t not in ("ByParam",)]
        item = {"path": m.group(1), "element": tags[-1] if tags else "", "line": i, "file": rel(p)}
        guard = re.search(r"\{\s*([\w.!&| ]+?)\s*&&\s*<Route", ln)
        if guard:
            item["guard"] = guard.group(1)
        out.append(item)
    return out


# ---------- база ----------

TABLE_RE = re.compile(
    r"\b(create|alter)\s+table\s+(?:if\s+(?:not\s+)?exists\s+)?(?:only\s+)?(?:public\.)?\"?(\w+)\"?",
    re.I,
)


def migrations() -> tuple[list[dict], list[dict]]:
    out = []
    tables: dict[str, dict] = {}
    for p in files("server/migrations", "*.sql"):
        text = p.read_text(encoding="utf-8")
        text_nc = re.sub(r"--[^\n]*", "", text)
        m = re.match(r"(\d+)_(.+)\.sql$", p.name)
        created, altered = set(), set()
        for kind, name in TABLE_RE.findall(text_nc):
            name = name.lower()
            (created if kind.lower() == "create" else altered).add(name)
        altered -= created
        num = m.group(1) if m else p.stem
        out.append(
            {
                "num": num,
                "name": m.group(2) if m else p.stem,
                "file": rel(p),
                "lines": text.count("\n") + 1,
                "creates": sorted(created),
                "alters": sorted(altered),
            }
        )
        for t in created:
            tables.setdefault(t, {"created_in": [], "altered_in": []})["created_in"].append(num)
        for t in altered:
            tables.setdefault(t, {"created_in": [], "altered_in": []})["altered_in"].append(num)
    tab_list = [
        {"table": t, "created_in": sorted(v["created_in"]), "altered_in": sorted(v["altered_in"])}
        for t, v in sorted(tables.items())
    ]
    return out, tab_list


# ---------- тесты ----------

RUST_TEST_ATTR_RE = re.compile(r"^\s*#\[(?:tokio::test|sqlx::test)\b")
JS_TEST_RE = re.compile(r"^(\s*)(test|it|describe)(?:\.(?:only|skip|fixme|serial|describe))?\(\s*(['\"`])(.*?)\3")


def tests() -> list[dict]:
    out = []
    for p in files("server/tests", "*.rs"):
        lines = read_lines(p)
        cases = []
        pending = False
        for i, ln in enumerate(lines, 1):
            if RUST_TEST_ATTR_RE.match(ln):
                pending = True
                continue
            if pending:
                m = re.match(r"^\s*(?:pub )?(?:async )?fn (\w+)", ln)
                if m:
                    cases.append({"name": m.group(1), "line": i})
                    pending = False
        out.append({"file": rel(p), "kind": "server", "lines": len(lines), "cases": cases})
    js = [(p, "e2e") for p in files("web/e2e", "*.ts")]
    js += [(p, "unit") for p in files("web/src", "**/*.test.ts") + files("web/src", "**/*.test.tsx")]
    for p, kind in sorted(js, key=lambda x: rel(x[0])):
        lines = read_lines(p)
        cases = []
        for i, ln in enumerate(lines, 1):
            m = JS_TEST_RE.match(ln)
            if m:
                item = {"name": short(m.group(4), 140), "line": i}
                if m.group(2) == "describe":
                    item["describe"] = True
                cases.append(item)
        if cases or kind == "e2e":
            out.append({"file": rel(p), "kind": kind, "lines": len(lines), "cases": cases})
    return out


# ---------- сборка ----------


def build() -> str:
    mig, tables = migrations()
    data = {
        "about": "Реестр-навигатор (scripts/registry.py). Строки — номера строк в файлах. Карта — docs/nav/NAV.md.",
        "spec": specs(),
        "adr": adrs(),
        "routes": routes(),
        "web_routes": web_routes(),
        "server_files": server_files(),
        "web_files": web_files(),
        "migrations": mig,
        "tables": tables,
        "tests": tests(),
    }
    # Один элемент списка — одна строка: файл остаётся читаемым jq и даёт короткие различия в git.
    parts = ["{"]
    keys = list(data.keys())
    for k_i, key in enumerate(keys):
        val = data[key]
        tail = "," if k_i + 1 < len(keys) else ""
        if isinstance(val, list):
            parts.append(f" {json.dumps(key)}: [")
            for i, item in enumerate(val):
                comma = "," if i + 1 < len(val) else ""
                parts.append("  " + json.dumps(item, ensure_ascii=False, sort_keys=True, separators=(",", ":")) + comma)
            parts.append(" ]" + tail)
        else:
            parts.append(f" {json.dumps(key)}: {json.dumps(val, ensure_ascii=False)}{tail}")
    parts.append("}")
    return "\n".join(parts) + "\n"


def main() -> int:
    check = "--check" in sys.argv[1:]
    content = build()
    current = OUT.read_text(encoding="utf-8") if OUT.exists() else None
    if check:
        if current != content:
            print(
                "registry: docs/nav/registry.json устарел — запустите python3 scripts/registry.py",
                file=sys.stderr,
            )
            return 1
        return 0
    if current != content:
        OUT.parent.mkdir(parents=True, exist_ok=True)
        OUT.write_text(content, encoding="utf-8", newline="\n")
        print(f"registry: записан {rel(OUT)} ({len(content.encode('utf-8'))} байт)")
    else:
        print("registry: без изменений")
    return 0


if __name__ == "__main__":
    sys.exit(main())
