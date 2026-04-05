/**
 * @module @darshan/nextjs/pages
 *
 * Pages Router helpers for DarshanDB. Provides idiomatic wrappers around
 * `getServerSideProps` and `getStaticProps` that inject the admin
 * DarshanDB and handle serialization.
 *
 * @example
 * ```tsx
 * // pages/users.tsx
 * import { queryServerSide } from '@darshan/nextjs/pages';
 *
 * export const getServerSideProps = queryServerSide(async (db, context) => {
 *   const users = await db.collection('users').find();
 *   return { users };
 * });
 *
 * export default function UsersPage({ users }: { users: User[] }) {
 *   return <UserList users={users} />;
 * }
 * ```
 *
 * @example
 * ```tsx
 * // pages/posts/[slug].tsx
 * import { queryStaticProps } from '@darshan/nextjs/pages';
 *
 * export const getStaticProps = queryStaticProps(
 *   async (db, context) => {
 *     const slug = context.params?.slug as string;
 *     const post = await db.collection('posts').findOne({ slug });
 *     if (!post) return null; // triggers notFound
 *     return { post };
 *   },
 *   { revalidate: 60 },
 * );
 * ```
 */

import type {
  GetServerSideProps,
  GetServerSidePropsContext,
  GetServerSidePropsResult,
  GetStaticProps,
  GetStaticPropsContext,
  GetStaticPropsResult,
} from 'next';
import type { DarshanDB } from '@darshan/client';
import type { ParsedUrlQuery } from 'querystring';
import { getAdminDb } from './server';

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/**
 * Callback for `queryServerSide`. Receives the admin DarshanDB
 * and the standard `GetServerSidePropsContext`.
 *
 * Return a plain object of props, or `null` to trigger a 404.
 */
export type ServerSideQueryFn<
  P extends Record<string, unknown> = Record<string, unknown>,
  Q extends ParsedUrlQuery = ParsedUrlQuery,
> = (
  db: DarshanDB,
  context: GetServerSidePropsContext<Q>,
) => Promise<P | null>;

/**
 * Callback for `queryStaticProps`. Receives the admin DarshanDB
 * and the standard `GetStaticPropsContext`.
 *
 * Return a plain object of props, or `null` to trigger a 404.
 */
export type StaticQueryFn<
  P extends Record<string, unknown> = Record<string, unknown>,
  Q extends ParsedUrlQuery = ParsedUrlQuery,
> = (
  db: DarshanDB,
  context: GetStaticPropsContext<Q>,
) => Promise<P | null>;

/** Options for `queryStaticProps`. */
export interface QueryStaticOptions {
  /**
   * ISR revalidation period in seconds.
   * - `false` — no revalidation (fully static)
   * - `number` — revalidate after N seconds
   */
  revalidate?: number | false;
}

// ---------------------------------------------------------------------------
// queryServerSide
// ---------------------------------------------------------------------------

/**
 * Wrap a DarshanDB query as a `getServerSideProps` function.
 *
 * The wrapper initializes the admin client from environment variables,
 * invokes your callback, and returns the result as Next.js props.
 * If the callback returns `null`, a 404 is served.
 *
 * @typeParam P - Shape of the props object returned to the page.
 * @typeParam Q - Shape of the URL query/params.
 * @param fn - Query callback receiving `(db, context)`.
 * @returns A `getServerSideProps` function ready for export.
 *
 * @example
 * ```ts
 * export const getServerSideProps = queryServerSide(async (db, ctx) => {
 *   const id = ctx.params?.id as string;
 *   const user = await db.collection('users').findOne({ _id: id });
 *   if (!user) return null;
 *   return { user };
 * });
 * ```
 */
export function queryServerSide<
  P extends Record<string, unknown> = Record<string, unknown>,
  Q extends ParsedUrlQuery = ParsedUrlQuery,
>(fn: ServerSideQueryFn<P, Q>): GetServerSideProps<P, Q> {
  return async (context) => {
    const db = getAdminDb();

    try {
      const result = await fn(db, context as GetServerSidePropsContext<Q>);

      if (result === null) {
        return { notFound: true } as GetServerSidePropsResult<P>;
      }

      // Ensure data is serializable (strip class instances, functions, etc.)
      const serialized = JSON.parse(JSON.stringify(result)) as P;

      return {
        props: serialized,
      };
    } catch (error) {
      const message =
        error instanceof Error ? error.message : 'Unknown error';
      console.error(`[DarshanDB] getServerSideProps error: ${message}`);
      throw error;
    }
  };
}

// ---------------------------------------------------------------------------
// queryStaticProps
// ---------------------------------------------------------------------------

/**
 * Wrap a DarshanDB query as a `getStaticProps` function.
 *
 * Supports ISR via the `revalidate` option. If the callback returns
 * `null`, a 404 is served.
 *
 * @typeParam P - Shape of the props object returned to the page.
 * @typeParam Q - Shape of the URL params.
 * @param fn - Query callback receiving `(db, context)`.
 * @param options - Static generation options (revalidation).
 * @returns A `getStaticProps` function ready for export.
 *
 * @example
 * ```ts
 * export const getStaticProps = queryStaticProps(
 *   async (db) => {
 *     const settings = await db.collection('settings').findOne({ key: 'global' });
 *     return { settings: settings ?? {} };
 *   },
 *   { revalidate: 300 },
 * );
 * ```
 */
export function queryStaticProps<
  P extends Record<string, unknown> = Record<string, unknown>,
  Q extends ParsedUrlQuery = ParsedUrlQuery,
>(fn: StaticQueryFn<P, Q>, options: QueryStaticOptions = {}): GetStaticProps<P, Q> {
  return async (context) => {
    const db = getAdminDb();

    try {
      const result = await fn(db, context as GetStaticPropsContext<Q>);

      if (result === null) {
        return { notFound: true } as GetStaticPropsResult<P>;
      }

      // Ensure data is serializable
      const serialized = JSON.parse(JSON.stringify(result)) as P;

      const staticResult: GetStaticPropsResult<P> = {
        props: serialized,
      };

      // Apply revalidation if specified
      if (options.revalidate !== undefined) {
        (staticResult as { revalidate?: number | false }).revalidate =
          options.revalidate;
      }

      return staticResult;
    } catch (error) {
      const message =
        error instanceof Error ? error.message : 'Unknown error';
      console.error(`[DarshanDB] getStaticProps error: ${message}`);
      throw error;
    }
  };
}
