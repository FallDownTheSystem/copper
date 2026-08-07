import { describe, expect, it } from 'vite-plus/test'

import { imageMime } from './imageMime'

function bytes(...values: number[]): ArrayBuffer {
	return Uint8Array.from(values).buffer
}

/** A header plus enough filler that a length check cannot be what passes. */
function header(...values: number[]): ArrayBuffer {
	return Uint8Array.from([...values, ...Array.from({ length: 32 }, () => 0)]).buffer
}

describe('imageMime', () => {
	it('names each of the five types Rust will hand over', () => {
		expect(imageMime(header(0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a))).toBe('image/png')
		expect(imageMime(header(0xff, 0xd8, 0xff, 0xe0))).toBe('image/jpeg')
		expect(imageMime(header(0x47, 0x49, 0x46, 0x38, 0x39, 0x61))).toBe('image/gif')
		expect(imageMime(header(0x42, 0x4d))).toBe('image/bmp')
		expect(
			// `RIFF` … `WEBP`, with the four length bytes in between.
			imageMime(header(0x52, 0x49, 0x46, 0x46, 1, 2, 3, 4, 0x57, 0x45, 0x42, 0x50)),
		).toBe('image/webp')
	})

	it('refuses a RIFF container that is not WebP', () => {
		// `RIFF` … `WAVE`. Claiming this as an image is what checking only the first
		// marker would do.
		expect(imageMime(header(0x52, 0x49, 0x46, 0x46, 1, 2, 3, 4, 0x57, 0x41, 0x56, 0x45))).toBeNull()
	})

	it('returns null rather than guessing at bytes it does not know', () => {
		expect(imageMime(header(0x25, 0x50, 0x44, 0x46))).toBeNull()
		expect(imageMime(bytes())).toBeNull()
	})

	it('does not read past the end of a truncated header', () => {
		// A two-byte buffer whose bytes are the start of the PNG signature: long
		// enough to tempt an unguarded comparison, too short to be one.
		expect(imageMime(bytes(0x89, 0x50))).toBeNull()
	})
})
