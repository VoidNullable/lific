export type Entity = 'issue' | 'page' | 'comment';
export interface Target {entity_type: Entity; entity_id: number;}
export interface Attachment {
  id: number;
  filename: string;
  mime: string;
  size_bytes: number;
  uploader_id: number | null;
  created_at: string;
  width?: number | null;
  height?: number | null;
  alt_text?: string | null;
  has_thumbnail?: boolean;
}
export interface UploadResponse {
  id: number;
  url: string;
  filename: string;
  mime: string;
  size: number;
  width?: number | null;
  height?: number | null;
  alt_text?: string | null;
  has_thumbnail?: boolean;
}
export type Preview = {kind: 'zip'; entries: Array<{name: string; size: number; compressed: number}>; total_entries: number; truncated: boolean}
  | {kind: 'sqlite'; tables: Array<{name: string; rows: number}>}
  | {kind: 'none'};
export type ViewerKind = 'image' | 'video' | 'audio' | 'diff' | 'csv' | 'json' | 'zip' | 'sqlite' | 'text' | 'file';
export interface Failure {ok: false; error: string; status: number | null; canceled?: boolean; code?: string | null; contentRange?: string | null;}
export type Result<T> = {ok: true; data: T; status?: number} | Failure;
export interface Progress {loaded: number; total: number | null;}
export interface UploadHandle {result: Promise<Result<UploadResponse>>; abort(): void;}
export interface DownloadMetadata {
  status: number;
  filename: string;
  contentType: string | null;
  contentRange: string | null;
  acceptRanges: string | null;
  contentLength: string | null;
}
export interface Destination {
  write(chunk: Uint8Array): void | Promise<void>;
  close(): void | Promise<void>;
  abort(reason: unknown): void | Promise<void>;
}
export type DownloadResult = ({ok: true; loaded: number} & DownloadMetadata) | Failure;
export interface DownloadOptions {
  open(metadata: DownloadMetadata): Destination | Promise<Destination>;
  variant?: 'original' | 'thumbnail';
  range?: string | null;
  filename?: string;
  signal?: AbortSignal;
  onProgress?(progress: Progress): void;
}
export interface Client {
  upload(file: File, options?: {target?: Target | null; onProgress?(progress: Progress): void}): UploadHandle;
  url(id: number, variant?: 'original' | 'thumbnail' | 'preview'): string;
  list(target: Target): Promise<Result<Attachment[]>>;
  preview(id: number, options?: {signal?: AbortSignal}): Promise<Result<Preview>>;
  remove(id: number, options?: {signal?: AbortSignal}): Promise<Result<{deleted: boolean}>>;
  altText(id: number, alt_text: string | null): Promise<Result<Attachment>>;
  text(id: number, options?: {signal?: AbortSignal}): Promise<({ok: true; text: string; loaded: number} & DownloadMetadata) | Failure>;
  thumbnail(id: number, options?: {signal?: AbortSignal}): Promise<({ok: true; blob: Blob; loaded: number} & DownloadMetadata) | Failure>;
  streamDownload(id: number, options: DownloadOptions): Promise<DownloadResult>;
  /** Scope/account marker; use only for detecting transitions, never display it. */
  audience(): string;
}
export interface Session {
  state: {publicProject: string | null; user: {id: number} | null};
  resolve(path: string, method?: string): {kind: 'private' | 'public'; url: string} | {kind: 'refused' | 'synthetic'};
  request<T>(path: string, options?: RequestInit): Promise<Result<T>>;
  clearSession(): void;
}
export interface Primitives {
  MAX_INLINE_BYTES: number;
  createClient(options: {session: Session; win?: Window; fetch?: typeof fetch; createXHR?(): XMLHttpRequest}): Client;
  viewerKind(attachment: {filename: string; mime?: string | null; size_bytes?: number}): ViewerKind;
  filename(contentDisposition: string | null, fallback?: string): string;
  markdown(attachment: Pick<UploadResponse, 'id' | 'filename' | 'mime' | 'alt_text'>): string;
  attach(root: HTMLElement, options: {client: Client; target?: Target | null; win?: Window; onUploaded?(row: UploadResponse, markdown: string): void | Promise<void>; onDeleted?(id: number): void | Promise<void>}): {dispose(): void};
}
declare global {var LificTopcoatAttachments: Primitives;}
