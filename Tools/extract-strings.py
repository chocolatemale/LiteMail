#!/usr/bin/env python3
"""Lists localizable string keys used in the Swift sources (with printf-style specifiers).
Run: python3 Tools/extract-strings.py [--missing app/Resources/zh-Hans.lproj/Localizable.strings]"""
import re, sys, pathlib
ROOT = pathlib.Path(__file__).resolve().parent.parent / "app" / "Sources" / "LiteMail"
START = re.compile(r'(?:\b(?:Text|Button|Label|Toggle|Section|Picker|LabeledContent|ContentUnavailableView|Tab|Menu|CommandMenu|WindowGroup|Link|SecureField|TextField)\(|String\(localized:\s*)"')
INT_HINT = re.compile(r'^(Int\(|(?!Sizes\.)[\w.]*\.count\b|secs\b|unread\b)')

def literal(src, i):
    """Parse a Swift string literal body starting after the opening quote; return (key, end)."""
    out = ""
    while True:
        c = src[i]
        if c == '"':
            return out, i + 1
        if c == "\\" and src[i + 1] == "(":
            depth, j, in_str = 1, i + 2, False
            while depth:
                ch = src[j]
                if in_str:
                    if ch == "\\": j += 1
                    elif ch == '"': in_str = False
                elif ch == '"': in_str = True
                elif ch == "(": depth += 1
                elif ch == ")": depth -= 1
                j += 1
            expr = src[i + 2:j - 1].strip()
            out += "%lld" if INT_HINT.match(expr) else "%@"
            i = j
            continue
        if c == "\\":
            nxt = src[i + 1]
            out += {"n": "\\n", '"': '\\"', "\\": "\\\\"}.get(nxt, nxt)
            i += 2
            continue
        out += c
        i += 1

keys = set()
for f in sorted(ROOT.rglob("*.swift")):
    if f.name == "Automation.swift":
        continue
    src = f.read_text()
    for m in START.finditer(src):
        k, _ = literal(src, m.end())
        if k.strip() and k not in ("%@",):
            keys.add(k)

if len(sys.argv) > 2 and sys.argv[1] == "--missing":
    have = set(re.findall(r'^"((?:[^"\\]|\\.)*)"\s*=', pathlib.Path(sys.argv[2]).read_text(), re.M))
    missing = sorted(keys - have)
    for k in missing: print(k)
    sys.exit(1 if missing else 0)
for k in sorted(keys): print(k)
