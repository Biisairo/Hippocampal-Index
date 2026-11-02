"""실측 transcript에서 user/assistant 라인만 남겨 테스트 fixture를 만든다."""
import json, sys, os
P = os.path.expanduser('~/.claude/projects/-Users-user-personal-session-db-probe/')
SRC = {
    's1_notool': '742660a8-64b5-4e03-9892-bd32ca4d5f59',
    's2_tools': 'afd070e6-4625-4b31-8c43-f1f553949222',
    's3_fg_agent': '24ae434f-f9cb-4ed9-9edf-98fede93c411',
    's4_bg_agent': '40c046b1-c6a7-4e0c-8d41-f78b1f948059',
    's5_bg_bash': '78507f8e-dc4c-49a8-8c47-a6e478d0ed71',
}
NOISE = '{"type":"attachment","attachment":{"type":"date","date":"2026-11-02"}}\n'
out = sys.argv[1]
for name, sid in SRC.items():
    with open(P + sid + '.jsonl') as f, open(f'{out}/{name}.jsonl', 'w') as w:
        w.write(NOISE)
        for line in f:
            if json.loads(line).get('type') in ('user', 'assistant'):
                w.write(line)
    sub = P + sid + '/subagents/'
    if os.path.isdir(sub):
        for fn in os.listdir(sub):
            if fn.endswith('.jsonl'):
                with open(sub + fn) as f, open(f'{out}/{name}.{fn}', 'w') as w:
                    for line in f:
                        if json.loads(line).get('type') in ('user', 'assistant'):
                            w.write(line)
