import { useState, useEffect, useCallback } from 'react';
import { SpeechAPI, type SegmentDiscardedEvent, type SegmentUpdatedEvent } from '../api/tauri-client';
import type { Segment } from '../api/tauri-client';
import { listen } from '@tauri-apps/api/event';

export type AppStatus = 'idle' | 'initializing' | 'recording' | 'processing' | 'error' | 'finished';

/// Stable ordering for the segment list. Backend assigns a monotonic
/// `revision` (orchestrator's segment id), so prefer that — it keeps
/// preloaded history and live segments in chronological order even if
/// their wall-clock strings disagree on format. Fall back to wall_start
/// then start_sec for the (rare) cases without a revision.
function compareSegments(a: Segment, b: Segment): number {
  const ar = a.revision ?? a.segment_id ?? a.id;
  const br = b.revision ?? b.segment_id ?? b.id;
  if (typeof ar === 'number' && typeof br === 'number' && ar !== br) {
    return ar - br;
  }
  if (a.wall_start !== b.wall_start) {
    return a.wall_start.localeCompare(b.wall_start);
  }
  return a.start - b.start;
}

export const useAppStore = () => {
  const [status, setStatus] = useState<AppStatus>('initializing');
  const [errorMessage, setErrorMessage] = useState<string>('');
  const [segments, setSegments] = useState<Segment[]>([]);
  const [devices, setDevices] = useState<{ label: string; value: string }[]>([]);
  const [selectedDevice, setSelectedDevice] = useState<string>('');
  const [showEnglish, setShowEnglish] = useState(true);
  const [isInitialized, setIsInitialized] = useState(false);

  const mapDbSegment = useCallback((row: Record<string, unknown>): Segment => ({
    id: typeof row.id === 'number' ? row.id : null,
    segment_id: typeof row.segment_id === 'number' ? row.segment_id : null,
    revision: typeof row.revision === 'number' ? row.revision : undefined,
    start: typeof row.start_sec === 'number' ? row.start_sec : 0,
    end: typeof row.end_sec === 'number' ? row.end_sec : 0,
    wall_start: typeof row.wall_start === 'string' ? row.wall_start : '',
    wall_end: typeof row.wall_end === 'string' ? row.wall_end : '',
    text_raw: typeof row.text_raw === 'string' ? row.text_raw : '',
    text_optimized: typeof row.text_optimized === 'string' ? row.text_optimized : undefined,
    text_english: typeof row.text_english === 'string' ? row.text_english : undefined,
    text_secondary: typeof row.text_secondary === 'string' ? row.text_secondary : undefined,
    secondary_kind: typeof row.secondary_kind === 'string' ? row.secondary_kind : undefined,
    speaker: typeof row.speaker === 'string' && row.speaker.length > 0 ? row.speaker : undefined,
    optimize_status:
      row.optimize_status === 'pending' ||
      row.optimize_status === 'running' ||
      row.optimize_status === 'success' ||
      row.optimize_status === 'failed'
        ? row.optimize_status
        : 'pending',
    translate_status:
      row.translate_status === 'blocked' ||
      row.translate_status === 'pending' ||
      row.translate_status === 'running' ||
      row.translate_status === 'success' ||
      row.translate_status === 'failed'
        ? row.translate_status
        : 'blocked',
  }), []);

  // Map an orchestrator /api/history row (server SegmentRow shape) into the
  // Segment shape the desktop UI consumes. Server fields:
  //   id, session_id, ts, text, optimized, english, speaker, has_audio
  // We use `id` as both segment_id and a synthetic revision so it merges
  // with live `segment_updated` events without colliding.
  const mapServerHistory = useCallback((row: Record<string, unknown>): Segment => {
    const id = typeof row.id === 'number' ? row.id : null;
    const ts = typeof row.ts === 'string' ? row.ts : '';
    return {
      id,
      segment_id: id,
      revision: id ?? undefined,
      start: 0,
      end: 0,
      wall_start: ts,
      wall_end: ts,
      text_raw: typeof row.text === 'string' ? row.text : '',
      text_optimized: typeof row.optimized === 'string' ? row.optimized : undefined,
      text_english: typeof row.english === 'string' ? row.english : undefined,
      text_secondary: typeof row.secondary === 'string' ? row.secondary : undefined,
      speaker: typeof row.speaker === 'string' && row.speaker.length > 0 ? row.speaker : undefined,
      // Server-side history is post-processing — mark both stages as
      // 'success' regardless of whether optimized/english are present, so
      // preloaded rows never show a spinner. SegmentCard falls back to
      // `text_raw` when the optimized field is empty, which is the right
      // affordance for "this is past context, not currently being processed".
      optimize_status: 'success',
      translate_status: 'success',
    };
  }, []);

  // Initialize
  useEffect(() => {
    let canceled = false;
    let initTimer: number | null = null;
    let unsubscribeSegmentDiscarded: (() => void) | null = null;
    let unsubscribeSegmentUpdated: (() => void) | null = null;

    const init = async () => {
      try {
        const deviceList = await SpeechAPI.listDevices();
        if (canceled) return;
        setDevices(deviceList.map(d => ({ label: d.is_default ? `${d.name} (Default)` : d.name, value: d.name })));

        const selected = await SpeechAPI.getSelectedDevice();
        if (canceled) return;
        if (selected) setSelectedDevice(selected);

        // Preload last 5 history segments from the orchestrator so the panel
        // isn't empty before the first recording of this session.
        try {
          const rows = await SpeechAPI.fetchRemoteHistory(5);
          if (canceled) return;
          if (rows.length > 0) {
            const mapped = rows
              .map(mapServerHistory)
              .filter((seg) => seg.text_raw.trim().length > 0)
              .reverse();
            setSegments(mapped);
          }
        } catch (err) {
          console.warn('Preload remote history failed', err);
        }

        pollInit();
      } catch (err) {
        if (canceled) return;
        console.error("Init failed", err);
        setStatus('error');
      }
    };

    const pollInit = async () => {
      if (canceled) return;
      const res = await SpeechAPI.getInitStatus();
      if (canceled) return;
      if (res.status === 1) {
        setStatus((prev) => (prev === 'initializing' ? 'idle' : prev));
      } else if (res.status === 2) {
        setErrorMessage(res.error || '初始化失败');
        setStatus('error');
      } else {
        initTimer = window.setTimeout(pollInit, 500);
      }
    };

    const runInit = async () => {
      await init();
      setIsInitialized(true);
    };
    runInit();

    // Subscribe to segment_discarded events
    void listen<SegmentDiscardedEvent>('segment_discarded', (event) => {
      if (canceled) return;
      const { revision, segment_id } = event.payload;
      console.debug('[segment_discarded]', { revision, segment_id, reason: event.payload.reason });

      setSegments((prev) => {
        const filtered = prev.filter(s => {
          if (segment_id !== null && s.segment_id === segment_id) return false;
          if (s.revision !== undefined && revision !== undefined && s.revision === revision) return false;
          return true;
        });

        return filtered.sort(compareSegments);
      });
    })
      .then((unlisten) => {
        if (canceled) { unlisten(); return; }
        unsubscribeSegmentDiscarded = unlisten;
      })
      .catch((err) => { console.error('Subscribe segment_discarded failed', err); });

    void listen<SegmentUpdatedEvent>('segment_updated', (event) => {
      if (canceled) return;
      const row = event.payload;
      const next = mapDbSegment(row as unknown as Record<string, unknown>);
      if (next.revision === undefined) return;
      console.debug('[segment_updated]', { revision: next.revision, segmentId: next.segment_id });
      setSegments((prev) => {
        const exists = prev.some((segment) => segment.revision === next.revision);
        const updated = exists
          ? prev.map((segment) =>
              segment.revision === next.revision ? { ...segment, ...next } : segment,
            )
          : [...prev, next];

        return updated.sort(compareSegments);
      });
    })
      .then((unlisten) => {
        if (canceled) { unlisten(); return; }
        unsubscribeSegmentUpdated = unlisten;
      })
      .catch((err) => { console.error('Subscribe segment_updated failed', err); });

    return () => {
      canceled = true;
      if (initTimer !== null) window.clearTimeout(initTimer);
      if (typeof unsubscribeSegmentDiscarded === 'function') unsubscribeSegmentDiscarded();
      if (typeof unsubscribeSegmentUpdated === 'function') unsubscribeSegmentUpdated();
    };
  }, [mapDbSegment, mapServerHistory]);

  return {
    status, setStatus,
    errorMessage, setErrorMessage,
    segments, setSegments,
    devices, setDevices,
    selectedDevice, setSelectedDevice,
    showEnglish, setShowEnglish,
    isInitialized,
  };
};
