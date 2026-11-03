// STUB - Prompt templates va ResourceLoader CHUA LAP DAY.
// Khung chi giu cho: prompt templates va context files cap qua ResourceLoader, theo prep muc 1.
export interface ResourceLoader {
  load(name: string): Promise<string>;
}

// CHUA CO: ResourceLoader that. Khong tra ve noi dung gia.
export function createResourceLoader(): ResourceLoader {
  throw new Error("ResourceLoader CHUA LAP DAY.");
}
