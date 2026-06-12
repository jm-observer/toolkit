import React, { useEffect, useRef } from 'react';
import { cn } from '../utils';

interface WaveformProps {
  active: boolean;
  intensity?: number;
  className?: string;
}

export const Waveform: React.FC<WaveformProps> = ({ active, intensity = 1, className }) => {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const bars = 32;
  const barWidth = 3;
  const gap = 2;
  const heights = useRef(new Array(bars).fill(2));
  const requestRef = useRef<number>(0);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const ctx = canvas.getContext('2d');
    if (!ctx) return;

    const render = () => {
      ctx.clearRect(0, 0, canvas.width, canvas.height);
      const primaryColor = getComputedStyle(document.documentElement).getPropertyValue('--primary').trim() || '#1aa181';
      const ink4Color = getComputedStyle(document.documentElement).getPropertyValue('--ink-4').trim() || '#a4ada8';

      ctx.fillStyle = active ? primaryColor : ink4Color;
      ctx.globalAlpha = active ? 1 : 0.3;

      for (let i = 0; i < bars; i++) {
        if (active) {
          const target = 4 + Math.random() * (canvas.height - 8) * intensity;
          heights.current[i] += (target - heights.current[i]) * 0.2;
        } else {
          heights.current[i] += (2 - heights.current[i]) * 0.1;
        }

        const h = heights.current[i];
        const x = i * (barWidth + gap);
        const y = (canvas.height - h) / 2;

        ctx.beginPath();
        ctx.roundRect(x, y, barWidth, h, 2);
        ctx.fill();
      }

      requestRef.current = requestAnimationFrame(render);
    };

    render();
    return () => cancelAnimationFrame(requestRef.current);
  }, [active, intensity]);

  return (
    <canvas
      ref={canvasRef}
      width={bars * (barWidth + gap) - gap}
      height={64}
      className={cn("w-full max-w-[160px]", className)}
    />
  );
};
