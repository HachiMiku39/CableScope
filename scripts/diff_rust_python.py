"""字段级差分：Python 版 `run.py --json` 与 Rust CLI `--json` 的快照比对。

用法：python3 scripts/diff_rust_python.py <python.json> <rust.json> [--verbose]

规则：
- 顶层 `at`（时间戳）忽略；`errors` 按集合比较（顺序无关）。
- `devices`/`ports`/`fabric`/`displays` 数组按规范化 JSON 排序后比较
  （枚举顺序不构成语义差异）；其余字段深度相等。
- 任何差异打印到 stdout 并以退出码 1 结束；一致时打印通过摘要。
"""

import json
import sys

IGNORE_TOP = {"at"}
ORDER_FREE_LISTS = {"devices", "ports", "fabric", "displays", "diagnostics"}


def canonical(value):
    return json.dumps(value, ensure_ascii=False, sort_keys=True)


def diff(path, left, right, out):
    if isinstance(left, dict) and isinstance(right, dict):
        for key in sorted(set(left) | set(right)):
            if key not in left:
                out.append("%s: 仅 Rust 侧存在 %r" % (path + "." + key, right[key]))
            elif key not in right:
                out.append("%s: 仅 Python 侧存在 %r" % (path + "." + key, left[key]))
            elif key in ORDER_FREE_LISTS and isinstance(left[key], list) and isinstance(right[key], list):
                l_list = sorted((canonical(v) for v in left[key]))
                r_list = sorted((canonical(v) for v in right[key]))
                if l_list != r_list:
                    out.append("%s.%s: 列表内容不同" % (path, key))
                    if len(left[key]) != len(right[key]):
                        out.append("  长度 %d != %d" % (len(left[key]), len(right[key])))
            elif key == "errors" and isinstance(left[key], list) and isinstance(right[key], list):
                if sorted(map(str, left[key])) != sorted(map(str, right[key])):
                    out.append("%s.errors: 错误集合不同" % path)
            else:
                diff(path + "." + key, left[key], right[key], out)
        return
    if isinstance(left, list) and isinstance(right, list):
        if len(left) != len(right):
            out.append("%s: 列表长度 %d != %d" % (path, len(left), len(right)))
            return
        for index, (l_item, r_item) in enumerate(zip(left, right)):
            diff("%s[%d]" % (path, index), l_item, r_item, out)
        return
    if left != right:
        out.append("%s: %r != %r" % (path, left, right))


def main():
    verbose = "--verbose" in sys.argv
    args = [a for a in sys.argv[1:] if a != "--verbose"]
    if len(args) != 2:
        print(__doc__)
        return 2
    with open(args[0], encoding="utf-8") as stream:
        python_side = json.load(stream)
    with open(args[1], encoding="utf-8") as stream:
        rust_side = json.load(stream)
    for key in IGNORE_TOP:
        python_side.pop(key, None)
        rust_side.pop(key, None)
    out = []
    diff("$", python_side, rust_side, out)
    if out:
        print("DIFF: %d 处不一致" % len(out))
        for line in out[:50]:
            print(line)
        if len(out) > 50:
            print("... 其余 %d 处省略" % (len(out) - 50))
        return 1
    print("一致：快照字段级比对通过（忽略顶层 at；devices/ports/fabric/displays 顺序无关）")
    if verbose:
        print("platform=%s ports=%d devices=%d errors=%d" % (
            rust_side.get("platform"), len(rust_side.get("ports", [])),
            len(rust_side.get("devices", [])), len(rust_side.get("errors", []))))
    return 0


if __name__ == "__main__":
    sys.exit(main())
