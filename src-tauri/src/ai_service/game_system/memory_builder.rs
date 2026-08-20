use std::collections::HashSet;

use crate::ai_service::types::{GameLine, LineBase, LlmMessage};
use crate::db::entities::line::LineAttribute;

/// 将 `GameLine` 序列构建成目标角色的 LLM 消息列表。
pub struct MemoryBuilder {
    pub target_role_id: i32,
}

enum BufferKind {
    TargetAssistant,
    OtherBlock,
}

impl MemoryBuilder {
    pub fn new(target_role_id: i32) -> Self {
        Self { target_role_id }
    }

    fn is_target(&self, line: &GameLine) -> bool {
        if line.sender_role_id() == Some(self.target_role_id) {
            return true;
        }
        line.perceived_role_ids.contains(&self.target_role_id)
    }

    /// 格式化内容：【情绪】内容（动作）<TTS>，仅用于 assistant (AI自身) 消息。
    fn format_content_with_extras(&self, line: &LineBase) -> String {
        let mut s = String::new();
        if let Some(emo) = line.original_emotion.as_deref().filter(|v| !v.is_empty()) {
            s.push('【');
            s.push_str(emo);
            s.push('】');
        }
        s.push_str(&line.content);
        s.push('\n');
        if let Some(act) = line.action_content.as_deref().filter(|v| !v.is_empty()) {
            s.push('(');
            s.push_str(act);
            s.push(')');
            s.push('\n');
        }

        if let Some(tts) = line.tts_content.as_deref().filter(|v| !v.is_empty()) {
            s.push('<');
            s.push_str(tts);
            s.push('>');
            s.push('\n');
        }

        s.push('\n');

        s
    }

    /// [修改点 1]：格式化为 context 行：过滤掉情绪和TTS，仅保留 "名称: 内容(动作)"
    fn format_context_line(&self, line: &LineBase) -> String {
        let name = line.display_name.as_deref().unwrap_or("未知");
        let mut s = match name {
            "旁白" | "系统" => line.content.clone(),
            _ => format!("{}: {}", name, line.content),
        };

        // 如果有动作，则追加 (动作)
        if let Some(act) = line.action_content.as_deref().filter(|v| !v.is_empty()) {
            s.push('(');
            s.push_str(act);
            s.push(')');
        }
        s
    }

    pub fn build(&self, lines: &[GameLine]) -> Vec<LlmMessage> {
        let mut memory: Vec<LlmMessage> = Vec::new();
        let mut buffer: Vec<GameLine> = Vec::new();
        let mut buffer_kind: Option<BufferKind> = None;

        let flush = |memory: &mut Vec<LlmMessage>,
                     buffer: &mut Vec<GameLine>,
                     buffer_kind: &mut Option<BufferKind>,
                     this: &MemoryBuilder| {
            if buffer.is_empty() {
                *buffer_kind = None;
                return;
            }
            match buffer_kind {
                Some(BufferKind::TargetAssistant) => {
                    let full: String = buffer
                        .iter()
                        .map(|l| this.format_content_with_extras(&l.base))
                        .collect();
                    if !full.trim().is_empty() {
                        memory.push(LlmMessage::assistant(full));
                    }
                }
                Some(BufferKind::OtherBlock) => {
                    // 从末尾向前找连续的 user 行，切分 context / active_user
                    let mut split_index = buffer.len();
                    for i in (0..buffer.len()).rev() {
                        let is_user = matches!(buffer[i].attribute(), LineAttribute::User);
                        if !is_user {
                            split_index = i + 1;
                            break;
                        }
                        if i == 0 && is_user {
                            split_index = 0;
                        }
                    }
                    let (context_lines, active_user_lines) = buffer.split_at(split_index);

                    let mut parts: Vec<String> = Vec::new();

                    // 记录是否包含上下文（即是否有其他角色发言）
                    let has_context = !context_lines.is_empty();

                    if has_context {
                        let joined: Vec<String> = context_lines
                            .iter()
                            .map(|l| this.format_context_line(&l.base))
                            .collect();
                        parts.push(format!("{{{}}}", joined.join("\n")));
                    }

                    if !active_user_lines.is_empty() {
                        // [修改点 2]：如果存在其他角色台词(has_context)，则强制给 User 台词加上 "主角名称: "
                        let user_text: Vec<String> = active_user_lines
                            .iter()
                            .map(|l| {
                                let name = l.base.display_name.as_deref().unwrap_or("未知");
                                let s = match name {
                                    "旁白" | "系统" => l.base.content.clone(),
                                    _ => format!("{}: {}", name, l.base.content),
                                };
                                s
                            })
                            .collect();
                        // 用换行符拼接多条User台词
                        parts.push(user_text.join("\n"));
                    }

                    let final_content =
                        if !context_lines.is_empty() && !active_user_lines.is_empty() {
                            parts.join("\n")
                        } else {
                            parts.concat()
                        };
                    memory.push(LlmMessage::user(final_content));
                }
                None => {}
            }
            buffer.clear();
            *buffer_kind = None;
        };

        let mut has_system_for_target = false;

        // 上下文内「已发出、尚未收到结果」的工具调用 id 集合。用于两件事：
        // 1) 丢弃裁剪窗口起点切开配对导致的孤儿 tool 结果（其 assistant(tool_call) 在窗外）；
        // 2) 收尾时剥离结果缺失的悬空 tool_calls，保证发给 LLM 的调用-结果配对完整。
        let mut open_tool_call_ids: HashSet<String> = HashSet::new();

        for line in lines {
            // system 消息处理逻辑保持不变...
            if matches!(line.attribute(), LineAttribute::System) {
                if line.sender_role_id() == Some(self.target_role_id) {
                    flush(&mut memory, &mut buffer, &mut buffer_kind, self);
                    if has_system_for_target {
                        tracing::warn!(
                            "[MemoryBuilder] 角色 {} 存在多条 System 台词，已跳过重复项 \
                             (sender_role_id={})",
                            self.target_role_id,
                            line.sender_role_id().unwrap_or(-1)
                        );
                    } else {
                        has_system_for_target = true;
                        memory.push(LlmMessage::system(line.content().to_string()));
                    }
                }
                continue;
            }

            // 工具调用 assistant 行：优先读 tool_call 字段，兼容旧版 \n\n 内嵌格式
            if matches!(line.attribute(), LineAttribute::Assistant) {
                let has_tool_call = line
                    .base
                    .tool_call
                    .as_deref()
                    .map(|s| !s.is_empty())
                    .unwrap_or(false);

                if has_tool_call {
                    // 新版：tool_call 存 JSON，content 纯文本
                    if let Ok(tool_calls) =
                        serde_json::from_str::<Vec<crate::ai_service::types::ToolCall>>(
                            line.base.tool_call.as_deref().unwrap_or(""),
                        )
                    {
                        flush(&mut memory, &mut buffer, &mut buffer_kind, self);
                        register_tool_call_ids(&mut open_tool_call_ids, &tool_calls);
                        memory.push(LlmMessage {
                            role: "assistant".into(),
                            content: line.base.content.clone(),
                            tool_calls: Some(tool_calls),
                            tool_call_id: None,
                        });
                        continue;
                    }
                } else if !line.base.content.is_empty() {
                    // 旧版兼容：content = "tool_calls_json\n\ntext"
                    let (tool_calls_json, text) = if let Some(idx) = line.base.content.find("\n\n")
                    {
                        let (head, tail) = line.base.content.split_at(idx);
                        (head, tail.strip_prefix("\n\n").unwrap_or(""))
                    } else {
                        (line.base.content.as_str(), "")
                    };
                    if let Ok(tool_calls) = serde_json::from_str::<
                        Vec<crate::ai_service::types::ToolCall>,
                    >(tool_calls_json)
                    {
                        flush(&mut memory, &mut buffer, &mut buffer_kind, self);
                        register_tool_call_ids(&mut open_tool_call_ids, &tool_calls);
                        memory.push(LlmMessage {
                            role: "assistant".into(),
                            content: text.to_string(),
                            tool_calls: Some(tool_calls),
                            tool_call_id: None,
                        });
                        continue;
                    }
                }
            }

            // 工具返回行：content 存 JSON {"tool_call_id":..., "result":...}
            if matches!(line.attribute(), LineAttribute::Tool) {
                flush(&mut memory, &mut buffer, &mut buffer_kind, self);
                let (tool_call_id, result) =
                    serde_json::from_str::<serde_json::Value>(&line.base.content)
                        .ok()
                        .map(|v| {
                            (
                                v.get("tool_call_id")
                                    .and_then(|s| s.as_str())
                                    .map(String::from),
                                v.get("result").map(|r| r.to_string()).unwrap_or_default(),
                            )
                        })
                        .unwrap_or((None, line.base.content.clone()));
                // 仅保留在本上下文内有配对 assistant(tool_call) 的工具结果；
                // 裁剪窗口起点切开配对时会出现孤儿 tool 消息，直接丢弃，
                // 避免发给 LLM 的对话中混入无配对的 tool 角色消息（provider 会拒绝）。
                let paired = match tool_call_id.as_deref() {
                    Some(id) => open_tool_call_ids.remove(id),
                    None => false,
                };
                if paired {
                    memory.push(LlmMessage {
                        role: "tool".into(),
                        content: result,
                        tool_calls: None,
                        tool_call_id,
                    });
                }
                continue;
            }

            if !self.is_target(line) {
                continue;
            }

            let is_self_speaking = (line.sender_role_id() == Some(self.target_role_id)
                && line.attribute() == &LineAttribute::Assistant);
            if is_self_speaking {
                if matches!(buffer_kind, Some(BufferKind::OtherBlock)) {
                    flush(&mut memory, &mut buffer, &mut buffer_kind, self);
                }
                buffer_kind = Some(BufferKind::TargetAssistant);
                buffer.push(line.clone());
            } else {
                if matches!(buffer_kind, Some(BufferKind::TargetAssistant)) {
                    flush(&mut memory, &mut buffer, &mut buffer_kind, self);
                }
                buffer_kind = Some(BufferKind::OtherBlock);
                buffer.push(line.clone());
            }
        }

        flush(&mut memory, &mut buffer, &mut buffer_kind, self);

        // 收尾：剥离「结果缺失」的悬空工具调用（如窗口在助手工具调用后切开、结果未写入）。
        // 规则：若某条 assistant 消息的任一 tool_call 在本上下文内未收到对应 tool 结果，
        // 整条剥离 tool_calls（保留正文），保证 provider 要求的调用-结果配对完整。
        if !open_tool_call_ids.is_empty() {
            for msg in memory.iter_mut() {
                if msg.role != "assistant" {
                    continue;
                }
                let has_missing = msg
                    .tool_calls
                    .as_ref()
                    .map_or(false, |calls| calls.iter().any(|c| open_tool_call_ids.contains(&c.id)));
                if has_missing {
                    msg.tool_calls = None;
                }
            }
        }

        memory
    }
}

/// 把一次助手工具调用的全部 call id 登记进 `open_tool_call_ids`，供后续
/// tool 结果行配对校验与收尾剥离使用。空 id（provider 异常）不登记。
fn register_tool_call_ids(open: &mut HashSet<String>, calls: &[crate::ai_service::types::ToolCall]) {
    for call in calls {
        if !call.id.trim().is_empty() {
            open.insert(call.id.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_service::types::LineAttributeExt;

    /// 构造目标角色(1) 的用户消息行。
    fn user_line(content: &str) -> GameLine {
        let mut base = LineBase::default();
        base.content = content.to_string();
        base.attribute = LineAttributeExt(LineAttribute::User);
        base.sender_role_id = Some(0);
        GameLine::from_base(base, vec![1])
    }

    /// 构造目标角色(1) 的普通助手正文行。
    fn assistant_line(content: &str) -> GameLine {
        let mut base = LineBase::default();
        base.content = content.to_string();
        base.attribute = LineAttributeExt(LineAttribute::Assistant);
        base.sender_role_id = Some(1);
        GameLine::from_base(base, vec![1])
    }

    /// 构造助手工具调用行（新版：tool_call 字段存 JSON，content 为调用前正文）。
    fn tool_call_line(id: &str, content: &str) -> GameLine {
        let mut base = LineBase::default();
        base.content = content.to_string();
        base.tool_call = Some(format!(
            r#"[{{"id":"{id}","function":{{"name":"web_search","description":"","parameters":{{}}}}}}]"#
        ));
        base.attribute = LineAttributeExt(LineAttribute::Assistant);
        GameLine::from_base(base, vec![1])
    }

    /// 构造工具结果行（content 为 {"tool_call_id","result"} JSON）。
    fn tool_result_line(id: &str, result: &str) -> GameLine {
        let mut base = LineBase::default();
        base.content = serde_json::json!({ "tool_call_id": id, "result": result }).to_string();
        base.attribute = LineAttributeExt(LineAttribute::Tool);
        GameLine::from_base(base, vec![1])
    }

    /// 完整工具轮次（调用→结果→正文）应原样构建，配对完整。
    #[test]
    fn complete_tool_round_is_built_in_order() {
        let builder = MemoryBuilder::new(1);
        let lines = vec![
            user_line("帮我查天气"),
            tool_call_line("call_1", "我来查一下"),
            tool_result_line("call_1", "晴天 25°C"),
            assistant_line("【开心】今天 25 度晴天"),
        ];
        let msgs = builder.build(&lines);
        let roles: Vec<&str> = msgs.iter().map(|m| m.role.as_str()).collect();
        assert_eq!(roles, vec!["user", "assistant", "tool", "assistant"]);
        assert!(msgs[1].tool_calls.is_some());
        assert_eq!(msgs[2].tool_call_id.as_deref(), Some("call_1"));
    }

    /// 窗口起点切开配对：孤儿 tool 结果（无配对 assistant(tool_call)）应被丢弃，
    /// 避免发给 LLM 的对话中出现无配对的 tool 角色消息（provider 会拒绝）。
    #[test]
    fn orphan_tool_result_is_dropped() {
        let builder = MemoryBuilder::new(1);
        let lines = vec![tool_result_line("call_9", "晴天 25°C"), assistant_line("【开心】25 度")];
        let msgs = builder.build(&lines);
        assert!(msgs.iter().all(|m| m.role != "tool"));
        assert_eq!(msgs.last().map(|m| m.role.as_str()), Some("assistant"));
    }

    /// 窗口在助手工具调用后切开、结果未写入：悬空 tool_calls 应被剥离（保留正文）。
    #[test]
    fn dangling_tool_calls_are_stripped() {
        let builder = MemoryBuilder::new(1);
        let lines = vec![user_line("帮我查天气"), tool_call_line("call_2", "我来查一下")];
        let msgs = builder.build(&lines);
        let last = msgs.last().unwrap();
        assert_eq!(last.role, "assistant");
        assert!(last.tool_calls.is_none());
    }

    /// 多结果轮次：一次调用多个工具，结果逐个配对后正常保留。
    #[test]
    fn multi_result_round_pairs_all() {
        let builder = MemoryBuilder::new(1);
        let lines = vec![
            user_line("查两个东西"),
            tool_call_line("a", ""),
            tool_call_line("b", ""),
            tool_result_line("a", "结果A"),
            tool_result_line("b", "结果B"),
            assistant_line("【思考】查到啦"),
        ];
        let msgs = builder.build(&lines);
        let roles: Vec<&str> = msgs.iter().map(|m| m.role.as_str()).collect();
        assert_eq!(roles, vec!["user", "assistant", "assistant", "tool", "tool", "assistant"]);
    }
}
