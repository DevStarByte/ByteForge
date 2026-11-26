using System.Numerics;
using System.Security.Cryptography;
using System.Text;

namespace ByteForge.FakeData;

/// <summary>
/// Python's random.Random: the Mersenne Twister (MT19937) seeded as CPython seeds it, with its getrandbits,
/// random(), randint, choice and uniform. A seeded generator gives exactly the values the Python module gave
/// for the same seed ("fakeData.seed": the n-th insertion stays the same across the port).
/// </summary>
public sealed class PyRandom
{
    const int N = 624, M = 397;
    const uint MatrixA = 0x9908b0df, UpperMask = 0x80000000, LowerMask = 0x7fffffff;

    readonly uint[] mt = new uint[N];
    int mti = N + 1;

    /// <summary>Seeded from the operating system's randomness (Python's Random(None)).</summary>
    public PyRandom()
    {
        var key = new uint[N];
        var bytes = RandomNumberGenerator.GetBytes(N * 4);
        Buffer.BlockCopy(bytes, 0, key, 0, bytes.Length);
        InitByArray(key);
    }

    /// <summary>Random(text): version 2 seeding, the text's UTF-8 bytes followed by their SHA-512, as one big-endian integer.</summary>
    public PyRandom(string seed)
    {
        var data = Encoding.UTF8.GetBytes(seed);
        SeedInteger(new BigInteger([.. data, .. SHA512.HashData(data)], isUnsigned: true, isBigEndian: true));
    }

    /// <summary>Random(n) for an int: all bits of |n|.</summary>
    public PyRandom(BigInteger seed) => SeedInteger(BigInteger.Abs(seed));

    void SeedInteger(BigInteger n)
    {
        // 32-bit words, least significant first (at least one).
        var bytes = n.ToByteArray(isUnsigned: true, isBigEndian: false);
        var words = Math.Max(1, (bytes.Length + 3) / 4);
        var padded = new byte[words * 4];
        bytes.CopyTo(padded, 0);
        var key = new uint[words];
        for (var i = 0; i < words; i++)
        {
            key[i] = BitConverter.ToUInt32(padded, i * 4);
        }
        // BigInteger.ToByteArray(isUnsigned) can add a zero byte; CPython uses ((bits - 1) / 32 + 1) words.
        var bits = (int)n.GetBitLength();
        var used = bits == 0 ? 1 : (bits - 1) / 32 + 1;
        InitByArray(key[..used]);
    }

    void InitGenrand(uint s)
    {
        mt[0] = s;
        for (mti = 1; mti < N; mti++)
        {
            mt[mti] = 1812433253U * (mt[mti - 1] ^ (mt[mti - 1] >> 30)) + (uint)mti;
        }
    }

    void InitByArray(uint[] key)
    {
        InitGenrand(19650218U);
        int i = 1, j = 0;
        for (var k = Math.Max(N, key.Length); k > 0; k--)
        {
            mt[i] = (mt[i] ^ ((mt[i - 1] ^ (mt[i - 1] >> 30)) * 1664525U)) + key[j] + (uint)j;
            i++;
            j++;
            if (i >= N)
            {
                mt[0] = mt[N - 1];
                i = 1;
            }
            if (j >= key.Length)
            {
                j = 0;
            }
        }
        for (var k = N - 1; k > 0; k--)
        {
            mt[i] = (mt[i] ^ ((mt[i - 1] ^ (mt[i - 1] >> 30)) * 1566083941U)) - (uint)i;
            i++;
            if (i >= N)
            {
                mt[0] = mt[N - 1];
                i = 1;
            }
        }
        mt[0] = 0x80000000U;
    }

    uint NextUInt32()
    {
        uint y;
        if (mti >= N)
        {
            int kk;
            for (kk = 0; kk < N - M; kk++)
            {
                y = (mt[kk] & UpperMask) | (mt[kk + 1] & LowerMask);
                mt[kk] = mt[kk + M] ^ (y >> 1) ^ ((y & 1) != 0 ? MatrixA : 0);
            }
            for (; kk < N - 1; kk++)
            {
                y = (mt[kk] & UpperMask) | (mt[kk + 1] & LowerMask);
                mt[kk] = mt[kk + (M - N)] ^ (y >> 1) ^ ((y & 1) != 0 ? MatrixA : 0);
            }
            y = (mt[N - 1] & UpperMask) | (mt[0] & LowerMask);
            mt[N - 1] = mt[M - 1] ^ (y >> 1) ^ ((y & 1) != 0 ? MatrixA : 0);
            mti = 0;
        }
        y = mt[mti++];
        y ^= y >> 11;
        y ^= (y << 7) & 0x9d2c5680U;
        y ^= (y << 15) & 0xefc60000U;
        y ^= y >> 18;
        return y;
    }

    /// <summary>random(): a float in [0, 1) with 53 random bits.</summary>
    public double Random()
    {
        uint a = NextUInt32() >> 5, b = NextUInt32() >> 6;
        return (a * 67108864.0 + b) * (1.0 / 9007199254740992.0);
    }

    /// <summary>getrandbits(k): 32-bit words from the least significant up, the last one cut to what's left.</summary>
    public BigInteger GetRandBits(int k)
    {
        if (k == 0)
        {
            return BigInteger.Zero;
        }
        if (k <= 32)
        {
            return NextUInt32() >> (32 - k);
        }
        var result = BigInteger.Zero;
        for (var shift = 0; k > 0; shift += 32, k -= 32)
        {
            var r = NextUInt32();
            if (k < 32)
            {
                r >>= 32 - k;
            }
            result |= new BigInteger(r) << shift;
        }
        return result;
    }

    /// <summary>_randbelow(n): getrandbits(n.bit_length()) until it's below n.</summary>
    public BigInteger RandBelow(BigInteger n)
    {
        var k = (int)n.GetBitLength();
        var r = GetRandBits(k);
        while (r >= n)
        {
            r = GetRandBits(k);
        }
        return r;
    }

    /// <summary>randint(a, b): a ≤ n ≤ b.</summary>
    public long RandInt(long a, long b) => a + (long)RandBelow(new BigInteger(b) - a + 1);

    public T Choice<T>(IReadOnlyList<T> items) =>
        items.Count == 0 ? throw new IndexOutOfRangeException("Cannot choose from an empty sequence") : items[(int)RandBelow(items.Count)];

    public double Uniform(double a, double b) => a + (b - a) * Random();
}
