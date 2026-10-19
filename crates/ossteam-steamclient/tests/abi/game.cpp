// SPDX-License-Identifier: GPL-2.0-or-later
//
// A small game for the ABI tests (SPEC §19.1 level 4): it loads the
// library as steam_api does and calls it through C++ vtables that the C++
// compiler lays out, not through the tables the shim generates.
//
// The declarations are our own, written from steamworks-abi.md and the
// interface pages: each class lists the slots it needs, in the order the
// Steamworks headers declare them, overloads included (§2.4). `CSteamID`
// is a class with constructors, so its return follows the C++ ABI's rule
// for classes (§2.2).
//
// Usage: game <path to the library>. Prints each failed check and exits
// with the number of failures.

#include <dlfcn.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <time.h>

static const uint64_t STEAM_ID = 76561197960287930ull;
static const uint64_t OTHER = 76561197960287931ull;

// Packed to 1 byte, as the SDK declares it (§2.2). Without that, GCC
// would keep its 8-byte alignment inside the callbacks' 4-byte packing,
// because the class has constructors.
#pragma pack(push, 1)
class CSteamID {
public:
    CSteamID() : bits(0) {}
    explicit CSteamID(uint64_t id) : bits(id) {}
    uint64_t ConvertToUint64() const { return bits; }

private:
    uint64_t bits;
};
#pragma pack(pop)

typedef int32_t HSteamPipe;
typedef int32_t HSteamUser;
typedef uint64_t SteamAPICall_t;

// Callbacks: 4 bytes on Linux and macOS (§2.2). A pragma's number must be
// written out: `pack(push, NAME)` pushes a label, not a macro's value.
#pragma pack(push, 4)
struct CallbackMsg_t {
    HSteamUser m_hSteamUser;
    int m_iCallback;
    uint8_t *m_pubParam;
    int m_cubParam;
};

struct SteamAPICallCompleted_t {
    SteamAPICall_t m_hAsyncCall;
    int m_iCallback;
    uint32_t m_cubParam;
};

struct UserStatsReceived_t {
    uint64_t m_nGameID;
    int m_eResult;
    CSteamID m_steamIDUser;
};
#pragma pack(pop)

class ISteamUser {
public:
    virtual HSteamUser GetHSteamUser() = 0;
    virtual bool BLoggedOn() = 0;
    virtual CSteamID GetSteamID() = 0;
};

class ISteamUtils {
public:
    virtual uint32_t GetSecondsSinceAppActive() = 0;
    virtual uint32_t GetSecondsSinceComputerActive() = 0;
    virtual int GetConnectedUniverse() = 0;
    virtual uint32_t GetServerRealTime() = 0;
    virtual const char *GetIPCountry() = 0;
    virtual bool GetImageSize(int iImage, uint32_t *pnWidth, uint32_t *pnHeight) = 0;
    virtual bool GetImageRGBA(int iImage, uint8_t *pubDest, int nDestBufferSize) = 0;
    virtual bool GetCSERIPPort(uint32_t *unIP, uint16_t *usPort) = 0;
    virtual uint8_t GetCurrentBatteryPower() = 0;
    virtual uint32_t GetAppID() = 0;
};

// STEAMUSERSTATS_INTERFACE_VERSION013, as far as GetUserStat.
class ISteamUserStats {
public:
    virtual bool GetStat(const char *pchName, int32_t *pData) = 0;
    virtual bool GetStat(const char *pchName, float *pData) = 0;
    virtual bool SetStat(const char *pchName, int32_t nData) = 0;
    virtual bool SetStat(const char *pchName, float fData) = 0;
    virtual bool UpdateAvgRateStat(const char *pchName, float flCountThisSession, double dSessionLength) = 0;
    virtual bool GetAchievement(const char *pchName, bool *pbAchieved) = 0;
    virtual bool SetAchievement(const char *pchName) = 0;
    virtual bool ClearAchievement(const char *pchName) = 0;
    virtual bool GetAchievementAndUnlockTime(const char *pchName, bool *pbAchieved, uint32_t *punUnlockTime) = 0;
    virtual bool StoreStats() = 0;
    virtual int GetAchievementIcon(const char *pchName) = 0;
    virtual const char *GetAchievementDisplayAttribute(const char *pchName, const char *pchKey) = 0;
    virtual bool IndicateAchievementProgress(const char *pchName, uint32_t nCurProgress, uint32_t nMaxProgress) = 0;
    virtual uint32_t GetNumAchievements() = 0;
    virtual const char *GetAchievementName(uint32_t iAchievement) = 0;
    virtual SteamAPICall_t RequestUserStats(CSteamID steamIDUser) = 0;
    virtual bool GetUserStat(CSteamID steamIDUser, const char *pchName, int32_t *pData) = 0;
    virtual bool GetUserStat(CSteamID steamIDUser, const char *pchName, float *pData) = 0;
};

// SteamClient020, as far as GetISteamUserStats.
class ISteamClient {
public:
    virtual HSteamPipe CreateSteamPipe() = 0;
    virtual bool BReleaseSteamPipe(HSteamPipe hSteamPipe) = 0;
    virtual HSteamUser ConnectToGlobalUser(HSteamPipe hSteamPipe) = 0;
    virtual HSteamUser CreateLocalUser(HSteamPipe *phSteamPipe, int eAccountType) = 0;
    virtual void ReleaseUser(HSteamPipe hSteamPipe, HSteamUser hUser) = 0;
    virtual ISteamUser *GetISteamUser(HSteamUser hSteamUser, HSteamPipe hSteamPipe, const char *pchVersion) = 0;
    virtual void *GetISteamGameServer(HSteamUser hSteamUser, HSteamPipe hSteamPipe, const char *pchVersion) = 0;
    virtual void SetLocalIPBinding(const void *unIP, uint16_t usPort) = 0;
    virtual void *GetISteamFriends(HSteamUser hSteamUser, HSteamPipe hSteamPipe, const char *pchVersion) = 0;
    virtual ISteamUtils *GetISteamUtils(HSteamPipe hSteamPipe, const char *pchVersion) = 0;
    virtual void *GetISteamMatchmaking(HSteamUser hSteamUser, HSteamPipe hSteamPipe, const char *pchVersion) = 0;
    virtual void *GetISteamMatchmakingServers(HSteamUser hSteamUser, HSteamPipe hSteamPipe, const char *pchVersion) = 0;
    virtual void *GetISteamGenericInterface(HSteamUser hSteamUser, HSteamPipe hSteamPipe, const char *pchVersion) = 0;
    virtual ISteamUserStats *GetISteamUserStats(HSteamUser hSteamUser, HSteamPipe hSteamPipe, const char *pchVersion) = 0;
};

typedef void *(*CreateInterfaceFn)(const char *, int *);
typedef bool (*BGetCallbackFn)(HSteamPipe, CallbackMsg_t *, int *);
typedef bool (*FreeLastCallbackFn)(HSteamPipe);
typedef bool (*GetAPICallResultFn)(HSteamPipe, SteamAPICall_t, void *, int, int, bool *);

static int failures = 0;

#define CHECK(cond)                                                    \
    do {                                                               \
        if (!(cond)) {                                                 \
            fprintf(stderr, "game.cpp:%d: failed: %s\n", __LINE__, #cond); \
            failures++;                                                \
        }                                                              \
    } while (0)

static BGetCallbackFn get_callback;
static FreeLastCallbackFn free_last_callback;

static void sleep_ms(long ms) {
    struct timespec t = {0, ms * 1000000L};
    nanosleep(&t, 0);
}

// Pumps callbacks until `id` arrives, copying it to `out`; false after
// five seconds.
static bool wait_callback(HSteamPipe pipe, int id, void *out, size_t size) {
    for (int i = 0; i < 5000; i++) {
        CallbackMsg_t msg;
        memset(&msg, 0, sizeof msg);
        if (get_callback(pipe, &msg, 0)) {
            bool match = msg.m_iCallback == id;
            if (match && (size_t)msg.m_cubParam != size) {
                fprintf(stderr, "callback %d has %d bytes, not %u\n", id, msg.m_cubParam, (unsigned)size);
                failures++;
            }
            if (match) {
                memcpy(out, msg.m_pubParam, size < (size_t)msg.m_cubParam ? size : (size_t)msg.m_cubParam);
            }
            CHECK(free_last_callback(pipe));
            if (match) {
                return true;
            }
        } else {
            sleep_ms(1);
        }
    }
    fprintf(stderr, "callback %d did not arrive\n", id);
    failures++;
    return false;
}

int main(int argc, char **argv) {
    if (argc != 2) {
        fprintf(stderr, "usage: game <library>\n");
        return 100;
    }
    void *lib = dlopen(argv[1], RTLD_NOW | RTLD_LOCAL);
    if (!lib) {
        fprintf(stderr, "dlopen: %s\n", dlerror());
        return 100;
    }
    CreateInterfaceFn create_interface = (CreateInterfaceFn)dlsym(lib, "CreateInterface");
    get_callback = (BGetCallbackFn)dlsym(lib, "Steam_BGetCallback");
    free_last_callback = (FreeLastCallbackFn)dlsym(lib, "Steam_FreeLastCallback");
    GetAPICallResultFn get_api_call_result = (GetAPICallResultFn)dlsym(lib, "Steam_GetAPICallResult");
    if (!create_interface || !get_callback || !free_last_callback || !get_api_call_result) {
        fprintf(stderr, "an export is missing\n");
        return 100;
    }

    // The client and a pipe (§2.3).
    int rc = -1;
    ISteamClient *client = (ISteamClient *)create_interface("SteamClient020", &rc);
    if (!client) {
        fprintf(stderr, "no SteamClient020\n");
        return 100;
    }
    CHECK(rc == 0);
    HSteamPipe pipe = client->CreateSteamPipe();
    HSteamUser user = client->ConnectToGlobalUser(pipe);
    if (pipe == 0 || user == 0) {
        fprintf(stderr, "no pipe (%d) or user (%d)\n", pipe, user);
        return 100;
    }

    // A class returned by value (§2.2).
    ISteamUser *steam_user = client->GetISteamUser(user, pipe, "SteamUser021");
    CHECK(steam_user != 0);
    if (steam_user) {
        CHECK(steam_user->GetHSteamUser() == user);
        CHECK(steam_user->BLoggedOn());
        CHECK(steam_user->GetSteamID().ConvertToUint64() == STEAM_ID);
    }

    ISteamUtils *utils = client->GetISteamUtils(pipe, "SteamUtils010");
    CHECK(utils != 0);
    if (utils) {
        CHECK(utils->GetAppID() == 480);
        CHECK(strcmp(utils->GetIPCountry(), "DE") == 0);
        CHECK(utils->GetServerRealTime() >= 1790000000u);
    }

    // Overloads in declaration order (§2.4). Version 013 asks for the
    // stats without being told to.
    ISteamUserStats *stats = client->GetISteamUserStats(user, pipe, "STEAMUSERSTATS_INTERFACE_VERSION013");
    CHECK(stats != 0);
    UserStatsReceived_t received = UserStatsReceived_t();
    if (stats && wait_callback(pipe, 1101, &received, sizeof received)) {
        CHECK(received.m_nGameID == 480);
        CHECK(received.m_eResult == 1);
        CHECK(received.m_steamIDUser.ConvertToUint64() == STEAM_ID);

        int32_t n = 0;
        float f = 0;
        CHECK(stats->GetStat("NumGames", &n));
        CHECK(n == 5);
        CHECK(!stats->GetStat("NumGames", &f));
        CHECK(stats->GetStat("FeetTraveled", &f));
        CHECK(f == 1.5f);
        CHECK(!stats->GetStat("FeetTraveled", &n));
        CHECK(stats->SetStat("NumGames", (int32_t)7));
        CHECK(!stats->SetStat("NumGames", 2.0f));
        CHECK(stats->SetStat("FeetTraveled", 2.5f));
        CHECK(stats->GetStat("NumGames", &n));
        CHECK(n == 7);
        CHECK(stats->GetStat("FeetTraveled", &f));
        CHECK(f == 2.5f);

        // A class passed by value, and a 64-bit result.
        SteamAPICall_t call = stats->RequestUserStats(CSteamID(OTHER));
        CHECK(call != 0);
        SteamAPICallCompleted_t done = SteamAPICallCompleted_t();
        if (wait_callback(pipe, 703, &done, sizeof done)) {
            CHECK(done.m_hAsyncCall == call);
            CHECK(done.m_iCallback == 1101);
            CHECK(done.m_cubParam == sizeof(UserStatsReceived_t));
            bool failed = true;
            received = UserStatsReceived_t();
            CHECK(get_api_call_result(pipe, call, &received, sizeof received, 1101, &failed));
            CHECK(!failed);
            CHECK(received.m_steamIDUser.ConvertToUint64() == OTHER);
            CHECK(stats->GetUserStat(CSteamID(OTHER), "NumGames", &n));
            CHECK(n == 9);
            CHECK(!stats->GetUserStat(CSteamID(OTHER), "NumGames", &f));
            CHECK(stats->GetUserStat(CSteamID(OTHER), "FeetTraveled", &f));
            CHECK(f == 1.5f);
        }
    }

    client->ReleaseUser(pipe, user);
    CHECK(client->BReleaseSteamPipe(pipe));
    if (failures == 0) {
        printf("ok\n");
    }
    return failures;
}
