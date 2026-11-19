import { describe, expect, it } from 'vitest';
import { endpoints, example, type OpenApi, snippets, typeLabel } from './openapi';

const spec: OpenApi = {
  info: { title: 'Test', version: '1' },
  paths: {
    '/projects/{id}': {
      get: { operationId: 'projects_get', tags: ['projects'] },
      patch: { operationId: 'projects_update', tags: ['projects'] },
    },
  },
  components: {
    schemas: {
      Member: {
        type: 'object',
        required: ['email', 'role'],
        properties: {
          email: { type: 'string', format: 'email' },
          role: { type: 'string', enum: ['developer', 'viewer'] },
          name: { type: ['string', 'null'] },
        },
      },
    },
  },
};

describe('openapi', () => {
  it('lists every operation with its method', () => {
    expect(endpoints(spec).map((e) => `${e.method} ${e.path}`)).toEqual(['get /projects/{id}', 'patch /projects/{id}']);
  });

  it('builds an example from the required fields only', () => {
    expect(example(spec, { type: 'array', items: { $ref: '#/components/schemas/Member' } })).toEqual([]);
    expect(example(spec, { $ref: '#/components/schemas/Member' })).toEqual({ email: 'anna@example.com', role: 'developer' });
  });

  it('names types for people', () => {
    expect(typeLabel(spec, { type: 'array', items: { $ref: '#/components/schemas/Member' } })).toBe('Member[]');
    expect(typeLabel(spec, { anyOf: [{ type: 'string', format: 'uuid' }, { type: 'null' }] })).toBe('string (uuid)');
  });

  it('quotes shell arguments safely', () => {
    const { curl, python } = snippets({
      method: 'post',
      url: 'https://x.test/api/v1/projects',
      body: `{"title":"Anna's shop","x":true}`,
    });
    expect(curl).toContain(`-d '{"title":"Anna'\\''s shop","x":true}'`);
    expect(python).toContain('"x": True');
  });
});
