// 语音指令通道前端类型定义。
// 设计权威：docs/voice-command-agent-design.md（§3/§5/§9）。

/** WS 连接状态机。 */
export type VoiceConnState =
  | 'idle'
  | 'connecting'
  | 'connected'
  | 'reconnecting'
  | 'closed';

/** 语音通道可配项（持久化到 plugin-store）。 */
export interface VoiceConfig {
  /** zero 语音 WS 端点，默认 ws://127.0.0.1:8101。 */
  url: string;
  /** 唤醒词变体表。 */
  wakeWords: string[];
  /** 常驻语音会话 id（连接时发一次 switch_session）；空则不发。 */
  sessionId: string;
}

/** zero 出站信封（§5.3）：纯文本回复另走 PlainReply。 */
export interface AudioEnvelope {
  type: 'audio';
  payload: {
    mime: string;
    data_base64: string;
    name?: string;
    message_id?: string;
  };
}

export interface FileEnvelope {
  type: 'file';
  payload: {
    mime: string;
    kind?: string;
    name?: string;
    message_id?: string;
    data_base64: string;
  };
}

export interface ErrorEnvelope {
  type: 'error';
  payload: string;
}

/** 客户端归一化后的回复事件。 */
export type VoiceReply =
  | { kind: 'text'; text: string }
  | { kind: 'audio'; mime: string; dataBase64: string; name?: string }
  | { kind: 'file'; mime: string; name?: string; dataBase64: string }
  | { kind: 'error'; message: string };

/** 一条「指令 → 回复」成对记录。 */
export interface VoiceExchange {
  id: string;
  /** 已剥唤醒词的指令文本。 */
  command: string;
  /** agent 回复（可能为空表示尚未到达）。 */
  replies: VoiceReply[];
  sentAt: number;
}
