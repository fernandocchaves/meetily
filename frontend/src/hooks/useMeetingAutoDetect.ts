import { useEffect } from 'react';
import { listen } from '@tauri-apps/api/event';
import { toast } from 'sonner';

/**
 * Bridges the Rust meeting_detector module's events to the existing
 * recording start/stop entry points.
 *
 * - 'auto-start-recording' reuses the same window event
 *   `useRecordingStart` already listens for when triggered from the
 *   sidebar (guards against double-start are handled there).
 * - 'auto-stop-recording' calls the `window.handleRecordingStop` hook that
 *   `useRecordingStop` exposes for Rust-originated stop requests.
 */
export function useMeetingAutoDetect() {
  useEffect(() => {
    const unlistenPromises = [
      listen<{ app_name: string; process_name: string }>('meeting-detected', (event) => {
        console.log('Meeting detected:', event.payload);
      }),
      listen('meeting-ended', () => {
        console.log('Meeting ended');
      }),
      listen<{ title: string; body: string }>('meeting-detection-notification', (event) => {
        toast.info(event.payload.title, {
          description: event.payload.body,
        });
      }),
      listen<{ meeting_name: string; app_name: string }>('auto-start-recording', (event) => {
        console.log('Auto-starting recording for:', event.payload);
        window.dispatchEvent(new Event('start-recording-from-sidebar'));
      }),
      listen('auto-stop-recording', () => {
        console.log('Auto-stopping recording');
        (window as any).handleRecordingStop?.(true);
      }),
    ];

    return () => {
      unlistenPromises.forEach((p) => p.then((unlisten) => unlisten()));
    };
  }, []);
}
