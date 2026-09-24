# 规则

- severity=hard scope=project value=* 成员只通过远程 Team MCP 读写业务状态；禁止用后端 SQL 改业务表做验收。
- severity=hard scope=project value=* 独立复核必须是与执行者不同的 person；禁止同一 agent 同时签署执行与复核。
- severity=hard scope=project value=* 客户端提示不得泄漏内部场景编号或夹具路径。
- severity=hard scope=project value=* 交付绑定真实 PR head 与可追溯回执。
