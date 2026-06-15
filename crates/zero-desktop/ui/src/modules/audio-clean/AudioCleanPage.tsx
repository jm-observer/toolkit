import { AudioCleanCard } from './AudioCleanCard';

/** 「音频清洗」独立页面（左侧导航一级入口）。来源是任意本地音/视频文件，
 *  与语音识别会话完全解耦。设计 docs/2026-06-15-audio-clean-standalone-menu/design.md。 */
export default function AudioCleanPage() {
  return (
    <div className="mx-auto max-w-3xl">
      <div className="mb-5">
        <h1 className="text-lg font-semibold text-[var(--ink-1)]">音频清洗</h1>
        <p className="mt-1 text-[13px] text-[var(--ink-3)]">
          去 BGM / 降噪 / 响度归一，处理任意本地音/视频文件，并列输出清洗后的副本（不覆盖原文件）。
        </p>
      </div>
      <AudioCleanCard />
    </div>
  );
}
