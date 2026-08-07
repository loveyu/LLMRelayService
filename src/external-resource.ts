import { request as httpRequest } from 'node:http';
import { request as httpsRequest } from 'node:https';
import { brotliDecompress, gunzip, inflate } from 'node:zlib';
import { promisify } from 'node:util';
import { ProxyAgent } from 'proxy-agent';

const SUPPORTED_PROXY_PROTOCOLS = new Set(['http:', 'https:', 'socks5:', 'socks5h:']);
const MAX_RESPONSE_BYTES = 20 * 1024 * 1024;
const gunzipAsync = promisify(gunzip);
const inflateAsync = promisify(inflate);
const brotliDecompressAsync = promisify(brotliDecompress);

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

async function decodeBody(body: Buffer, encoding: string | undefined): Promise<Buffer> {
  switch (encoding?.toLowerCase()) {
    case 'gzip':
      return gunzipAsync(body);
    case 'deflate':
      return inflateAsync(body);
    case 'br':
      return brotliDecompressAsync(body);
    default:
      return body;
  }
}

async function requestJsonThroughProxy(url: string, proxyUrl: string, timeoutMs: number, redirects = 0): Promise<unknown> {
  const target = new URL(url);
  const request = target.protocol === 'http:' ? httpRequest : httpsRequest;
  const agent = new ProxyAgent({ getProxyForUrl: () => proxyUrl });

  try {
    return await new Promise<unknown>((resolve, reject) => {
      let settled = false;
      let deadline: ReturnType<typeof setTimeout>;
      const finish = (callback: () => void) => {
        if (settled) return;
        settled = true;
        clearTimeout(deadline);
        callback();
      };
      const req = request(target, {
        agent,
        headers: {
          accept: 'application/json',
          'accept-encoding': 'br, gzip, deflate',
          'user-agent': 'LLMRelayService/1.0',
        },
      }, (response) => {
        const status = response.statusCode ?? 0;
        const location = response.headers.location;
        if (status >= 300 && status < 400 && location) {
          response.resume();
          if (redirects >= 3) {
            finish(() => reject(new Error(`External resource redirected too many times: ${url}`)));
            return;
          }
          void requestJsonThroughProxy(new URL(location, target).toString(), proxyUrl, timeoutMs, redirects + 1)
            .then((value) => finish(() => resolve(value)), (error) => finish(() => reject(error)));
          return;
        }
        if (status < 200 || status >= 300) {
          response.resume();
          finish(() => reject(new Error(`External resource responded with ${status}`)));
          return;
        }

        const chunks: Buffer[] = [];
        let size = 0;
        response.on('data', (chunk: Buffer | Uint8Array) => {
          size += chunk.length;
          if (size > MAX_RESPONSE_BYTES) {
            req.destroy(new Error(`External resource exceeded ${MAX_RESPONSE_BYTES} bytes`));
            return;
          }
          chunks.push(Buffer.from(chunk));
        });
        response.on('end', () => {
          void decodeBody(Buffer.concat(chunks), response.headers['content-encoding'])
            .then(
              (body) => finish(() => resolve(JSON.parse(body.toString('utf8')) as unknown)),
              (error) => finish(() => reject(error)),
            );
        });
        response.on('error', (error) => finish(() => reject(error)));
      });

      deadline = setTimeout(() => {
        req.destroy(new Error(`External resource timed out after ${timeoutMs}ms`));
      }, timeoutMs);
      req.on('error', (error) => finish(() => reject(error)));
      req.end();
    });
  } finally {
    agent.destroy();
  }
}

export async function fetchExternalJson(url: string, timeoutMs: number): Promise<unknown> {
  const proxyUrl = getExternalResourceProxyUrl();
  if (proxyUrl) return requestJsonThroughProxy(url, proxyUrl, timeoutMs);

  const response = await fetch(url, { signal: AbortSignal.timeout(timeoutMs) });
  if (!response.ok) throw new Error(`External resource responded with ${response.status}`);
  return response.json() as Promise<unknown>;
}
