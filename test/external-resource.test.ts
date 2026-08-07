import { afterEach, describe, expect, it } from 'bun:test';

import { getExternalResourceProxyUrl, redactProxyUrl } from '../src/external-resource';

const originalProxy = process.env.EXTERNAL_RESOURCE_PROXY;

afterEach(() => {
  if (originalProxy === undefined) delete process.env.EXTERNAL_RESOURCE_PROXY;
  else process.env.EXTERNAL_RESOURCE_PROXY = originalProxy;
});

describe('external resource proxy configuration', () => {
  it.each([
    'http://127.0.0.1:7890',
    'https://proxy.example.com:8443',
    'socks5://proxy.example.com:1080',
    'socks5h://user:password@proxy.example.com:1080',
  ])('accepts %s', (proxy) => {
    process.env.EXTERNAL_RESOURCE_PROXY = proxy;
    expect(getExternalResourceProxyUrl()).toBe(new URL(proxy).toString());
  });

  it('returns null when the setting is empty', () => {
    process.env.EXTERNAL_RESOURCE_PROXY = '  ';
    expect(getExternalResourceProxyUrl()).toBeNull();
  });

  it('rejects unsupported protocols and missing ports', () => {
    process.env.EXTERNAL_RESOURCE_PROXY = 'ftp://proxy.example.com:21';
    expect(() => getExternalResourceProxyUrl()).toThrow('protocol must be');

    process.env.EXTERNAL_RESOURCE_PROXY = 'http://proxy.example.com';
    expect(() => getExternalResourceProxyUrl()).toThrow('hostname and port');

    process.env.EXTERNAL_RESOURCE_PROXY = 'not a proxy URL';
    expect(() => getExternalResourceProxyUrl()).toThrow('valid proxy URL');
  });

  it('redacts proxy credentials from logs', () => {
    expect(redactProxyUrl('socks5://alice:secret@proxy.example.com:1080')).toBe(
      'socks5://***:***@proxy.example.com:1080',
    );
  });
});
