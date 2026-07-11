import React, { useState, useEffect } from 'react';
import { Switch } from '@/components/ui/switch';
import { Input } from '@/components/ui/input';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';

export interface WebhookSettings {
  enabled: boolean;
  url: string;
  secret: string | null;
}

const DEFAULT_SETTINGS: WebhookSettings = {
  enabled: false,
  url: '',
  secret: null,
};

export function WebhookSettings() {
  const [settings, setSettings] = useState<WebhookSettings>(DEFAULT_SETTINGS);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [testing, setTesting] = useState(false);

  useEffect(() => {
    const loadSettings = async () => {
      try {
        const loaded = await invoke<WebhookSettings>('get_webhook_settings');
        setSettings(loaded);
      } catch (error) {
        console.error('Failed to load webhook settings:', error);
      } finally {
        setLoading(false);
      }
    };
    loadSettings();
  }, []);

  const saveSettings = async (next: WebhookSettings) => {
    setSaving(true);
    setSettings(next);
    try {
      await invoke('set_webhook_settings', { settings: next });
      toast.success('Webhook settings saved');
    } catch (error) {
      console.error('Failed to save webhook settings:', error);
      toast.error('Failed to save webhook settings', {
        description: error instanceof Error ? error.message : String(error),
      });
    } finally {
      setSaving(false);
    }
  };

  const handleTestWebhook = async () => {
    setTesting(true);
    try {
      await invoke('test_webhook');
      toast.success('Test webhook sent');
    } catch (error) {
      console.error('Failed to send test webhook:', error);
      toast.error('Test webhook failed', {
        description: error instanceof Error ? error.message : String(error),
      });
    } finally {
      setTesting(false);
    }
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
        <h3 className="text-lg font-semibold mb-4">Webhook</h3>
        <p className="text-sm text-gray-600 mb-6">
          Send the full transcript of each finished meeting to an HTTP endpoint
          (e.g. your own agent) right after it's saved. This ships the raw
          transcript only — no summary is generated or sent.
        </p>
      </div>

      <div className="flex items-center justify-between p-4 border rounded-lg">
        <div className="flex-1">
          <div className="font-medium">Enable Webhook</div>
          <div className="text-sm text-gray-600">
            POST the transcript to the URL below when a recording finishes saving
          </div>
        </div>
        <Switch
          checked={settings.enabled}
          onCheckedChange={(checked) => saveSettings({ ...settings, enabled: checked })}
          disabled={saving}
        />
      </div>

      {settings.enabled && (
        <div className="space-y-4 p-4 border rounded-lg bg-gray-50">
          <div>
            <label className="text-sm font-medium block mb-2">Webhook URL</label>
            <Input
              type="url"
              placeholder="https://hermes.example.com/webhooks/meeting"
              value={settings.url}
              onChange={(e) => setSettings({ ...settings, url: e.target.value })}
              onBlur={() => saveSettings(settings)}
              disabled={saving}
            />
          </div>

          <div>
            <label className="text-sm font-medium block mb-2">
              Secret (optional)
            </label>
            <Input
              type="password"
              placeholder="Sent as: Authorization: Bearer <secret>"
              value={settings.secret ?? ''}
              onChange={(e) => setSettings({ ...settings, secret: e.target.value || null })}
              onBlur={() => saveSettings(settings)}
              disabled={saving}
            />
          </div>

          <button
            onClick={handleTestWebhook}
            disabled={testing || !settings.url}
            className="px-3 py-2 text-sm border border-gray-300 rounded-md hover:bg-white transition-colors disabled:opacity-50"
          >
            {testing ? 'Sending...' : 'Send test webhook'}
          </button>
        </div>
      )}
    </div>
  );
}
