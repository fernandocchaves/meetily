import React, { useState, useEffect } from 'react';
import { Switch } from '@/components/ui/switch';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';

export interface MeetingDetectionSettings {
  enabled: boolean;
  auto_start_recording: boolean;
  auto_stop_recording: boolean;
  detect_zoom: boolean;
  detect_teams: boolean;
  detect_google_meet: boolean;
  notify_on_detection: boolean;
  poll_interval_secs: number;
}

const DEFAULT_SETTINGS: MeetingDetectionSettings = {
  enabled: false,
  auto_start_recording: false,
  auto_stop_recording: true,
  detect_zoom: true,
  detect_teams: true,
  detect_google_meet: true,
  notify_on_detection: true,
  poll_interval_secs: 5,
};

export function MeetingDetectionSettings() {
  const [settings, setSettings] = useState<MeetingDetectionSettings>(DEFAULT_SETTINGS);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    const loadSettings = async () => {
      try {
        const loaded = await invoke<MeetingDetectionSettings>('get_meeting_detection_settings');
        setSettings(loaded);
      } catch (error) {
        console.error('Failed to load meeting detection settings:', error);
      } finally {
        setLoading(false);
      }
    };
    loadSettings();
  }, []);

  const saveSettings = async (next: MeetingDetectionSettings) => {
    setSaving(true);
    setSettings(next);
    try {
      await invoke('set_meeting_detection_settings', { settings: next });
      toast.success('Meeting detection settings saved');
    } catch (error) {
      console.error('Failed to save meeting detection settings:', error);
      toast.error('Failed to save meeting detection settings', {
        description: error instanceof Error ? error.message : String(error),
      });
    } finally {
      setSaving(false);
    }
  };

  const toggle = (key: keyof MeetingDetectionSettings) => (checked: boolean) => {
    saveSettings({ ...settings, [key]: checked });
  };

  if (loading) {
    return (
      <div className="animate-pulse">
        <div className="h-4 bg-gray-200 rounded w-1/4 mb-4"></div>
        <div className="h-8 bg-gray-200 rounded mb-4"></div>
      </div>
    );
  }

  return (
    <div className="space-y-6">
      <div>
        <h3 className="text-lg font-semibold mb-4">Meeting Detection</h3>
        <p className="text-sm text-gray-600 mb-6">
          Automatically detect when a meeting starts (Zoom, Teams, Google Meet) and
          start/stop recording without needing to click anything. Detection only
          watches for known process names running on your machine — no meeting
          content, audio, or video is accessed until recording actually starts.
        </p>
      </div>

      <div className="flex items-center justify-between p-4 border rounded-lg">
        <div className="flex-1">
          <div className="font-medium">Enable Meeting Detection</div>
          <div className="text-sm text-gray-600">
            Watch for Zoom, Teams, and Google Meet running on this machine
          </div>
        </div>
        <Switch checked={settings.enabled} onCheckedChange={toggle('enabled')} disabled={saving} />
      </div>

      {settings.enabled && (
        <>
          <div className="flex items-center justify-between p-4 border rounded-lg">
            <div className="flex-1">
              <div className="font-medium">Auto-Start Recording</div>
              <div className="text-sm text-gray-600">
                Start recording automatically when a meeting is detected
              </div>
            </div>
            <Switch
              checked={settings.auto_start_recording}
              onCheckedChange={toggle('auto_start_recording')}
              disabled={saving}
            />
          </div>

          <div className="flex items-center justify-between p-4 border rounded-lg">
            <div className="flex-1">
              <div className="font-medium">Auto-Stop Recording</div>
              <div className="text-sm text-gray-600">
                Stop recording automatically when the meeting ends
              </div>
            </div>
            <Switch
              checked={settings.auto_stop_recording}
              onCheckedChange={toggle('auto_stop_recording')}
              disabled={saving}
            />
          </div>

          <div className="flex items-center justify-between p-4 border rounded-lg">
            <div className="flex-1">
              <div className="font-medium">Notify on Detection</div>
              <div className="text-sm text-gray-600">
                Show a toast when a meeting is detected
              </div>
            </div>
            <Switch
              checked={settings.notify_on_detection}
              onCheckedChange={toggle('notify_on_detection')}
              disabled={saving}
            />
          </div>

          <div className="border-t pt-6 space-y-4">
            <h4 className="text-base font-medium text-gray-900">Apps to detect</h4>

            <div className="flex items-center justify-between p-4 border rounded-lg">
              <div className="font-medium">Zoom</div>
              <Switch checked={settings.detect_zoom} onCheckedChange={toggle('detect_zoom')} disabled={saving} />
            </div>

            <div className="flex items-center justify-between p-4 border rounded-lg">
              <div className="font-medium">Microsoft Teams</div>
              <Switch checked={settings.detect_teams} onCheckedChange={toggle('detect_teams')} disabled={saving} />
            </div>

            <div className="flex items-center justify-between p-4 border rounded-lg">
              <div className="flex-1">
                <div className="font-medium">Google Meet</div>
                <div className="text-sm text-gray-600">
                  Best-effort only — browser-based detection, less reliable than Zoom/Teams
                </div>
              </div>
              <Switch
                checked={settings.detect_google_meet}
                onCheckedChange={toggle('detect_google_meet')}
                disabled={saving}
              />
            </div>
          </div>
        </>
      )}
    </div>
  );
}
