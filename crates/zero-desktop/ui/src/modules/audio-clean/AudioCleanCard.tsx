import { useState } from 'react';
import { CleanAPI } from './api/clean-client';
import type { CleanOptions, CleanedRecording } from './api/clean-client';
import { Button } from '../speech/components/ui/Button';
import { Dropdown } from '../speech/components/ui/Dropdown';
import { Switch } from '../speech/components/ui/Switch';
import { Icon } from '../speech/components/ui/Icon';

const PAUSE_OPTIONS = [
  { label: '压低（duck）', value: 'duck' },
  { label: '删除（drop）', value: 'drop' },
  { label: '不处理（off）', value: 'off' },
];

const LEVEL_OPTIONS = [
  { label: '温和（gentle）', value: 'gentle' },
  { label: '均衡（balanced）', value: 'balanced' },
  { label: '激进（aggressive）', value: 'aggressive' },
];

const SR_OPTIONS = [
  { label: '48000 Hz', value: '48000' },
  { label: '24000 Hz', value: '24000' },
  { label: '16000 Hz', value: '16000' },
];

const FORMAT_OPTIONS = [
  { label: 'WAV', value: 'wav' },
  { label: 'MP3', value: 'mp3' },
  { label: 'FLAC', value: 'flac' },
];

/** 独立「音频清洗」卡片：选任意本地音/视频 → 去 BGM/降噪/响度归一 → 并列落盘。
 *  与 segment 识别流解耦（来源是任意文件而非当前会话录音）。 */
export function AudioCleanCard() {
  const [inputPath, setInputPath] = useState<string>('');
  const [denoise, setDenoise] = useState(true);
  const [separate, setSeparate] = useState(false);
  const [pause, setPause] = useState('duck');
  const [level, setLevel] = useState('balanced');
  const [sr, setSr] = useState('48000');
  const [format, setFormat] = useState('wav');

  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [result, setResult] = useState<CleanedRecording | null>(null);

  const handlePick = async () => {
    setError('');
    try {
      const picked = await CleanAPI.pickAudioFile();
      if (picked) {
        setInputPath(picked);
        setResult(null);
      }
    } catch (err) {
      setError(typeof err === 'string' ? err : (err as Error)?.message || String(err));
    }
  };

  const handleClean = async () => {
    if (!inputPath) return;
    setBusy(true);
    setError('');
    setResult(null);
    try {
      const opts: CleanOptions = {
        denoise,
        separate,
        pause,
        level,
        sr: Number(sr),
        format,
      };
      const res = await CleanAPI.cleanRecording(inputPath, opts);
      setResult(res);
    } catch (err) {
      setError(typeof err === 'string' ? err : (err as Error)?.message || String(err));
    } finally {
      setBusy(false);
    }
  };

  const handleOpenFolder = async () => {
    if (!result) return;
    try {
      await CleanAPI.openInFolder(result.cleaned_path);
    } catch (err) {
      console.error('open folder failed', err);
    }
  };

  const fmtLufs = (v: number) => (Number.isFinite(v) ? `${v.toFixed(1)} LUFS` : '—');

  return (
    <div className="rounded-2xl border border-[var(--line)] bg-[var(--bg-card)] p-5 shadow-[var(--shadow-sm)]">
      <div className="flex items-center gap-2 mb-4">
        <Icon name="wand" size={18} className="text-[var(--primary)]" />
        <h3 className="text-sm font-semibold text-[var(--ink-1)]">音频清洗</h3>
        <span className="text-[11px] text-[var(--ink-4)]">去 BGM / 降噪 / 响度归一</span>
      </div>

      {/* 文件选择 */}
      <div className="flex items-center gap-2 mb-4">
        <Button variant="outline" size="sm" onClick={handlePick} disabled={busy}>
          <Icon name="search" size={14} className="mr-1.5" />
          选择文件
        </Button>
        <span className="text-[12px] text-[var(--ink-3)] truncate flex-1" title={inputPath}>
          {inputPath || '未选择文件（支持 wav/mp3/m4a/flac/mp4/webm…）'}
        </span>
      </div>

      {/* 选项面板 */}
      <div className="grid grid-cols-2 gap-3 mb-4">
        <div className="flex items-center justify-between rounded-lg bg-[var(--bg-soft)] px-3 h-9">
          <span className="text-[12.5px] text-[var(--ink-2)]">降噪去混响</span>
          <Switch checked={denoise} onCheckedChange={setDenoise} disabled={busy} />
        </div>
        <div className="flex items-center justify-between rounded-lg bg-[var(--bg-soft)] px-3 h-9">
          <span className="text-[12.5px] text-[var(--ink-2)]">去 BGM（慢）</span>
          <Switch checked={separate} onCheckedChange={setSeparate} disabled={busy} />
        </div>
        <Dropdown label="停顿处理" options={PAUSE_OPTIONS} value={pause} onChange={setPause} disabled={busy} />
        <Dropdown label="降噪强度" options={LEVEL_OPTIONS} value={level} onChange={setLevel} disabled={busy} />
        <Dropdown label="采样率" options={SR_OPTIONS} value={sr} onChange={setSr} disabled={busy} />
        <Dropdown label="输出格式" options={FORMAT_OPTIONS} value={format} onChange={setFormat} disabled={busy} />
      </div>

      {/* 操作 */}
      <Button variant="primary" size="md" onClick={handleClean} disabled={busy || !inputPath} className="w-full">
        {busy ? (
          <>
            <Icon name="refresh" size={15} className="mr-2 animate-spin" />
            处理中，可能需要数分钟…
          </>
        ) : (
          <>
            <Icon name="wand" size={15} className="mr-2" />
            开始清洗
          </>
        )}
      </Button>

      {/* 结果 */}
      {result && (
        <div className="mt-4 rounded-lg border border-[var(--line)] bg-[var(--bg-soft)] p-3.5">
          <div className="flex items-center gap-2 mb-2 text-[var(--primary-deep)]">
            <Icon name="check" size={15} />
            <span className="text-[12.5px] font-medium">清洗完成</span>
          </div>
          <div className="text-[12px] text-[var(--ink-2)] break-all mb-2" title={result.cleaned_path}>
            {result.cleaned_path}
          </div>
          <div className="flex flex-wrap gap-x-4 gap-y-1 text-[11.5px] text-[var(--ink-3)] mb-2">
            <span>阶段：{result.stages.length ? result.stages.join(' → ') : '—'}</span>
            <span>响度：{fmtLufs(result.in_lufs)} → {fmtLufs(result.out_lufs)}</span>
          </div>
          <Button variant="soft" size="sm" onClick={handleOpenFolder}>
            <Icon name="search" size={13} className="mr-1.5" />
            打开所在文件夹
          </Button>
        </div>
      )}

      {/* 错误 */}
      {error && (
        <div className="mt-4 flex items-start gap-2 rounded-lg border border-[var(--danger)] bg-[var(--danger-soft)] p-3 text-[12px] text-[var(--danger)]">
          <Icon name="alert" size={15} className="mt-0.5 shrink-0" />
          <span className="break-all">{error}</span>
        </div>
      )}
    </div>
  );
}
