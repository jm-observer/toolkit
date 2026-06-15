// 录音转写 + 优化 + 翻译全部完成时的提示音。
//
// 用 Web Audio 即时合成一段两音“叮咚”，不依赖任何音频资源文件，保证可靠出声
// （后端的托盘闪烁 + MessageBeep 依赖系统“星号”声音方案，常被静音；前端合成音
// 与系统设置无关，更可靠）。所有异常吞掉——出不了声不该影响识别主流程。

let ctx: AudioContext | null = null

function audioCtx(): AudioContext | null {
  try {
    const Ctor: typeof AudioContext | undefined =
      window.AudioContext || (window as unknown as { webkitAudioContext?: typeof AudioContext }).webkitAudioContext
    if (!Ctor) return null
    if (!ctx) ctx = new Ctor()
    return ctx
  } catch {
    return null
  }
}

/** 播放一次完成提示音（两声短促上行音，约 0.4s）。 */
export function playCompletionSound(): void {
  const ac = audioCtx()
  if (!ac) return
  try {
    // 用户手势之外创建的 AudioContext 可能处于 suspended，尝试恢复。
    if (ac.state === 'suspended') void ac.resume()
    const now = ac.currentTime
    const tones = [880, 1175] // A5 → D6，悦耳的两音
    tones.forEach((freq, i) => {
      const osc = ac.createOscillator()
      const gain = ac.createGain()
      osc.type = 'sine'
      osc.frequency.value = freq
      const t = now + i * 0.16
      gain.gain.setValueAtTime(0.0001, t)
      gain.gain.exponentialRampToValueAtTime(0.18, t + 0.02)
      gain.gain.exponentialRampToValueAtTime(0.0001, t + 0.18)
      osc.connect(gain).connect(ac.destination)
      osc.start(t)
      osc.stop(t + 0.2)
    })
  } catch {
    /* 出声失败不影响主流程 */
  }
}
