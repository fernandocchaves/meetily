import { useEffect } from 'react';
import { listen } from '@tauri-apps/api/event';
import { toast } from 'sonner';

/**
 * Bridges the Rust meeting_detector module's events to the existing
 * recording start entry point.
 *
 * - 'auto-start-recording' reuses the same window event
 *   `useRecordingStart` already listens for when triggered from the
 *   sidebar (guards against double-start are handled there).
 *
 * Auto-stop needs no frontend wiring: the Rust side calls
 * `stop_recording` directly (same as the tray menu) and emits
 * 'recording-stop-complete', which `RecordingPostProcessingProvider`
 * already listens for globally.
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
        window.dispatchEvent(
          new CustomEvent('start-recording-from-sidebar', {
            detail: { meetingName: event.payload.meeting_name },
          })
        );
      }),
    ];

    return () => {
      unlistenPromises.forEach((p) => p.then((unlisten) => unlisten()));
    };
  }, []);
}
