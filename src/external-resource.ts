const SUPPORTED_PROXY_PROTOCOLS = new Set(['http:', 'https:', 'socks5:', 'socks5h:']);
const MAX_RESPONSE_BYTES = 20 * 1024 * 1024;

export function getExternalResourceProxyUrl(): string | null {
  const value = process.env.EXTERNAL_RESOURCE_PROXY?.trim();
  if (!value) return null;

  let url: URL;
  try {
    url = new URL(value);
  } catch {
    throw new Error('EXTERNAL_RESOURCE_PROXY must be a valid proxy URL');
  }
  if (!SUPPORTED_PROXY_PROTOCOLS.has(url.protocol)) {
    throw new Error(`EXTERNAL_RESOURCE_PROXY protocol must be http, https, socks5, or socks5h; received ${url.protocol}`);
  }
  if (!url.hostname || !url.port) {
    throw new Error('EXTERNAL_RESOURCE_PROXY must include a hostname and port');
  }
  return url.toString();
}

export function redactProxyUrl(value: string): string {
  const url = new URL(value);
  if (url.username || url.password) {
    url.username = '***';
    url.password = '***';
  }
  return url.toString();
}

function escapeCurlConfig(value: string): string {
  return value.replaceAll('\\', '\\\\').replaceAll('"', '\\"');
}

async function requestJsonThroughProxy(url: string, proxyUrl: string, timeoutMs: number): Promise<unknown> {
  const process = Bun.spawn([
    'curl',
    '--silent',
    '--show-error',
    '--fail-with-body',
    '--location',
    '--max-redirs', '3',
    '--max-time', String(Math.ceil(timeoutMs / 1000)),
    '--compressed',
    '--max-filesize', String(MAX_RESPONSE_BYTES),
    '--config', '-',
    url,
  ], {
    stdin: 'pipe',
    stdout: 'pipe',
    stderr: 'pipe',
  });

  process.stdin.write(`proxy = "${escapeCurlConfig(proxyUrl)}"\n`);
  process.stdin.end();

  const [exitCode, body, stderr] = await Promise.all([
    process.exited,
    new Response(process.stdout).arrayBuffer(),
    new Response(process.stderr).text(),
  ]);
  if (exitCode !== 0) {
    throw new Error(stderr.trim() || `curl exited with code ${exitCode}`);
  }
  if (body.byteLength > MAX_RESPONSE_BYTES) {
    throw new Error(`External resource exceeded ${MAX_RESPONSE_BYTES} bytes`);
  }
  return JSON.parse(Buffer.from(body).toString('utf8')) as unknown;
}

export async function fetchExternalJson(url: string, timeoutMs: number): Promise<unknown> {
  const proxyUrl = getExternalResourceProxyUrl();
  if (proxyUrl) return requestJsonThroughProxy(url, proxyUrl, timeoutMs);

  const response = await fetch(url, { signal: AbortSignal.timeout(timeoutMs) });
  if (!response.ok) throw new Error(`External resource responded with ${response.status}`);
  return response.json() as Promise<unknown>;
}
