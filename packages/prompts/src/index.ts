// STUB - Prompt templates and ResourceLoader NOT FILLED IN YET.
// The frame only reserves the place: prompt templates and context files served
// via ResourceLoader, per prep section 1.
export interface ResourceLoader {
  load(name: string): Promise<string>;
}

// NOT HERE YET: real ResourceLoader. No fake content is returned.
export function createResourceLoader(): ResourceLoader {
  throw new Error("ResourceLoader NOT FILLED IN YET.");
}
