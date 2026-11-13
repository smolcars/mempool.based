"""Writes a JUnit report's ten slowest tests as a Markdown table, for a CI job's summary."""

import sys
import xml.etree.ElementTree as ET

try:
    cases = ET.parse(sys.argv[1]).getroot().iter("testcase")
except (OSError, ET.ParseError) as e:
    print(f"No test times: {e}")
    sys.exit(0)
rows = sorted(((float(c.get("time") or 0), c.get("classname", ""), c.get("name", "")) for c in cases), reverse=True)
print(f"### Ten slowest of {len(rows)} tests ({sum(r[0] for r in rows):.0f} s in all)\n")
print("| Seconds | Where | Test |\n|--:|---|---|")
for secs, where, name in rows[:10]:
    print(f"| {secs:.1f} | `{where}` | {name.replace('|', '/')} |")
