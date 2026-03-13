#define _GNU_SOURCE

#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <signal.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <unistd.h>

static void die(const char *fmt, ...) {
    va_list args;
    va_start(args, fmt);
    vfprintf(stderr, fmt, args);
    fputc('\n', stderr);
    va_end(args);
    exit(1);
}

static void warn_message(const char *fmt, ...) {
    va_list args;
    va_start(args, fmt);
    vfprintf(stderr, fmt, args);
    fputc('\n', stderr);
    va_end(args);
}

static char *xstrdup(const char *value) {
    char *copy = strdup(value);
    if (copy == NULL) {
        die("AppRun: out of memory");
    }
    return copy;
}

static char *path_join(const char *base, const char *suffix) {
    size_t base_len = strlen(base);
    size_t suffix_len = strlen(suffix);
    char *result = malloc(base_len + suffix_len + 1);
    if (result == NULL) {
        die("AppRun: out of memory");
    }

    memcpy(result, base, base_len);
    memcpy(result + base_len, suffix, suffix_len + 1);
    return result;
}

static char *append_colon_list(char *current, const char *entry) {
    size_t current_len = strlen(current);
    size_t entry_len = strlen(entry);
    char *result = realloc(current, current_len + 1 + entry_len + 1);
    if (result == NULL) {
        free(current);
        die("AppRun: out of memory");
    }

    result[current_len] = ':';
    memcpy(result + current_len + 1, entry, entry_len + 1);
    return result;
}

static char *build_libpaths(const char *appdir) {
    static const char *suffixes[] = {
        "/usr/lib/x86_64-linux-gnu",
        "/lib/x86_64",
    };

    char *paths = path_join(appdir, suffixes[0]);
    for (size_t i = 1; i < sizeof(suffixes) / sizeof(suffixes[0]); ++i) {
        char *entry = path_join(appdir, suffixes[i]);
        paths = append_colon_list(paths, entry);
        free(entry);
    }
    return paths;
}

static void set_prefixed_env(const char *name, const char *value, const char *existing) {
    size_t value_len = strlen(value);
    size_t total_len = value_len + (existing != NULL ? 1 + strlen(existing) : 0) + 1;
    char *buffer = malloc(total_len);
    if (buffer == NULL) {
        die("AppRun: out of memory");
    }

    memcpy(buffer, value, value_len);
    if (existing != NULL && existing[0] != '\0') {
        buffer[value_len] = ':';
        strcpy(buffer + value_len + 1, existing);
    } else {
        buffer[value_len] = '\0';
    }

    if (setenv(name, buffer, 1) != 0) {
        free(buffer);
        die("AppRun: failed to set %s", name);
    }
    free(buffer);
}

static void set_env(const char *name, const char *value) {
    if (setenv(name, value, 1) != 0) {
        die("AppRun: failed to set %s", name);
    }
}

static void set_numeric_env(const char *name, long value) {
    char buffer[32];
    snprintf(buffer, sizeof(buffer), "%ld", value);
    set_env(name, buffer);
}

static int ensure_directory(const char *path, mode_t mode) {
    char *mutable = xstrdup(path);
    size_t len = strlen(mutable);
    if (len == 0) {
        free(mutable);
        return 0;
    }

    for (size_t i = 1; i < len; ++i) {
        if (mutable[i] != '/') {
            continue;
        }
        mutable[i] = '\0';
        if (mkdir(mutable, mode) != 0 && errno != EEXIST) {
            free(mutable);
            return -1;
        }
        mutable[i] = '/';
    }

    if (mkdir(mutable, mode) != 0 && errno != EEXIST) {
        free(mutable);
        return -1;
    }

    free(mutable);
    return 0;
}

static int copy_file(const char *source, const char *destination, mode_t mode) {
    FILE *input = fopen(source, "rb");
    if (input == NULL) {
        return -1;
    }

    FILE *output = fopen(destination, "wb");
    if (output == NULL) {
        fclose(input);
        return -1;
    }

    char buffer[16384];
    size_t bytes_read;
    int ok = 0;
    while ((bytes_read = fread(buffer, 1, sizeof(buffer), input)) > 0) {
        if (fwrite(buffer, 1, bytes_read, output) != bytes_read) {
            ok = -1;
            break;
        }
    }

    if (ferror(input)) {
        ok = -1;
    }

    if (fclose(output) != 0) {
        ok = -1;
    }
    fclose(input);

    if (ok == 0 && chmod(destination, mode) != 0) {
        ok = -1;
    }

    return ok;
}

static int is_wayland_session(void) {
    const char *wayland_display = getenv("WAYLAND_DISPLAY");
    if (wayland_display != NULL && wayland_display[0] != '\0') {
        return 1;
    }

    const char *session_type = getenv("XDG_SESSION_TYPE");
    return session_type != NULL && strcmp(session_type, "wayland") == 0;
}

static int desktop_integration_enabled(void) {
    const char *disable = getenv("ENZIMCODER_APPIMAGE_NO_DESKTOP_INTEGRATION");
    if (disable != NULL && disable[0] != '\0' && strcmp(disable, "0") != 0) {
        return 0;
    }
    return is_wayland_session();
}

static char *desktop_quote_exec_arg(const char *value) {
    size_t value_len = strlen(value);
    char *quoted = malloc(value_len * 2 + 3);
    if (quoted == NULL) {
        die("AppRun: out of memory");
    }

    size_t out = 0;
    quoted[out++] = '"';
    for (size_t i = 0; i < value_len; ++i) {
        char c = value[i];
        if (c == '\\' || c == '"' || c == '$' || c == '`') {
            quoted[out++] = '\\';
        }
        quoted[out++] = c;
    }
    quoted[out++] = '"';
    quoted[out] = '\0';
    return quoted;
}

static char *write_desktop_integration(const char *appdir) {
    if (!desktop_integration_enabled()) {
        return NULL;
    }

    const char *appimage_env = getenv("APPIMAGE");
    const char *home = getenv("HOME");
    if (appimage_env == NULL || appimage_env[0] == '\0' || home == NULL || home[0] == '\0') {
        return NULL;
    }

    char *appimage_path = realpath(appimage_env, NULL);
    if (appimage_path == NULL) {
        appimage_path = xstrdup(appimage_env);
    }

    const char *xdg_data_home_env = getenv("XDG_DATA_HOME");
    char *xdg_data_home = (xdg_data_home_env != NULL && xdg_data_home_env[0] != '\0')
        ? xstrdup(xdg_data_home_env)
        : path_join(home, "/.local/share");
    char *applications_dir = path_join(xdg_data_home, "/applications");
    char *icons_root_dir = path_join(xdg_data_home, "/icons");
    char *icon_dir = path_join(xdg_data_home, "/icons/hicolor/512x512/apps");
    char *scalable_icon_dir = path_join(xdg_data_home, "/icons/hicolor/scalable/apps");
    char *desktop_path = path_join(applications_dir, "/dev.enzim.EnzimCoder.desktop");
    char *png_destination = path_join(icon_dir, "/dev.enzim.EnzimCoder.png");
    char *svg_destination = path_join(scalable_icon_dir, "/dev.enzim.EnzimCoder.svg");
    char *png_source = path_join(appdir, "/dev.enzim.EnzimCoder.png");
    char *svg_source = path_join(appdir, "/dev.enzim.EnzimCoder.svg");
    char *quoted_exec = desktop_quote_exec_arg(appimage_path);
    char *desktop_tmp = path_join(applications_dir, "/dev.enzim.EnzimCoder.desktop.tmp");
    char *result = NULL;

    if (ensure_directory(applications_dir, 0755) != 0 ||
        ensure_directory(icons_root_dir, 0755) != 0 ||
        ensure_directory(icon_dir, 0755) != 0 ||
        ensure_directory(scalable_icon_dir, 0755) != 0) {
        warn_message("AppRun: failed to create user desktop integration directories");
        goto cleanup;
    }

    if (copy_file(png_source, png_destination, 0644) != 0) {
        warn_message("AppRun: failed to install AppImage icon at %s", png_destination);
        goto cleanup;
    }
    if (copy_file(svg_source, svg_destination, 0644) != 0) {
        warn_message("AppRun: failed to install AppImage icon at %s", svg_destination);
        goto cleanup;
    }

    FILE *desktop = fopen(desktop_tmp, "wb");
    if (desktop == NULL) {
        warn_message("AppRun: failed to write desktop entry at %s", desktop_path);
        goto cleanup;
    }

    fprintf(
        desktop,
        "[Desktop Entry]\n"
        "Type=Application\n"
        "Name=Enzim Coder\n"
        "Comment=Local-first AI coding workspace with threads, Git, and files\n"
        "Exec=%s %%U\n"
        "Icon=%s\n"
        "Terminal=false\n"
        "Categories=Development;IDE;\n"
        "Keywords=codex;coding;chat;git;workspace;\n"
        "StartupNotify=true\n"
        "StartupWMClass=dev.enzim.EnzimCoder\n"
        "X-EnzimCoder-AppImage-Managed=true\n",
        quoted_exec,
        png_destination
    );

    if (fclose(desktop) != 0 || chmod(desktop_tmp, 0644) != 0 || rename(desktop_tmp, desktop_path) != 0) {
        warn_message("AppRun: failed to finalize desktop entry at %s", desktop_path);
        unlink(desktop_tmp);
        goto cleanup;
    }

    result = xstrdup(desktop_path);

cleanup:
    free(desktop_tmp);
    free(quoted_exec);
    free(svg_source);
    free(png_source);
    free(svg_destination);
    free(png_destination);
    free(desktop_path);
    free(scalable_icon_dir);
    free(icon_dir);
    free(icons_root_dir);
    free(applications_dir);
    free(xdg_data_home);
    free(appimage_path);
    return result;
}

static void maybe_detach_for_terminal_launch(void) {
    const char *disable_detach = getenv("ENZIMCODER_APPIMAGE_NO_DETACH");
    if (disable_detach != NULL && disable_detach[0] != '\0' && strcmp(disable_detach, "0") != 0) {
        return;
    }

    if (!isatty(STDIN_FILENO) && !isatty(STDOUT_FILENO) && !isatty(STDERR_FILENO)) {
        return;
    }

    pid_t pid = fork();
    if (pid < 0) {
        die("AppRun: failed to fork for detach");
    }
    if (pid > 0) {
        exit(0);
    }

    if (setsid() < 0) {
        die("AppRun: failed to create detached session");
    }

    signal(SIGHUP, SIG_IGN);

    int null_fd = open("/dev/null", O_RDWR);
    if (null_fd < 0) {
        die("AppRun: failed to open /dev/null");
    }

    if (dup2(null_fd, STDIN_FILENO) < 0 || dup2(null_fd, STDOUT_FILENO) < 0 || dup2(null_fd, STDERR_FILENO) < 0) {
        close(null_fd);
        die("AppRun: failed to detach stdio");
    }

    if (null_fd > STDERR_FILENO) {
        close(null_fd);
    }
}

int main(int argc, char **argv) {
    char self_path[PATH_MAX];
    ssize_t len = readlink("/proc/self/exe", self_path, sizeof(self_path) - 1);
    if (len < 0) {
        die("AppRun: failed to resolve /proc/self/exe");
    }
    self_path[len] = '\0';

    char *appdir = xstrdup(self_path);
    char *last_slash = strrchr(appdir, '/');
    if (last_slash == NULL) {
        free(appdir);
        die("AppRun: invalid launcher path");
    }
    *last_slash = '\0';

    char *libpaths = build_libpaths(appdir);
    char *binary = path_join(appdir, "/usr/bin/enzimcoder");
    char *desktop_file = path_join(appdir, "/dev.enzim.EnzimCoder.desktop");
    char *installed_desktop_file = write_desktop_integration(appdir);

    set_env("APPDIR", appdir);
    {
        char *xdg_data_dirs = path_join(appdir, "/usr/share");
        char *local_share = path_join(appdir, "/usr/local/share");
        xdg_data_dirs = append_colon_list(xdg_data_dirs, local_share);
        set_prefixed_env("XDG_DATA_DIRS", xdg_data_dirs, getenv("XDG_DATA_DIRS"));
        free(local_share);
        free(xdg_data_dirs);
    }
    set_prefixed_env("XDG_CONFIG_DIRS", path_join(appdir, "/etc/xdg"), getenv("XDG_CONFIG_DIRS"));

    {
        char *path_prefix = path_join(appdir, "/etc/init.d");
        static const char *path_suffixes[] = {
            "/usr/bin",
            "/usr/lib/dbus-1.0",
            "/usr/lib/x86_64-linux-gnu/gdk-pixbuf-2.0",
            "/usr/lib/x86_64-linux-gnu/glib-2.0",
            "/usr/lib/x86_64-linux-gnu/gstreamer1.0/gstreamer-1.0",
            "/usr/libexec",
            "/usr/libexec/glycin-loaders/2+",
            "/usr/libexec/libselinux",
            "/usr/sbin",
        };
        for (size_t i = 0; i < sizeof(path_suffixes) / sizeof(path_suffixes[0]); ++i) {
            char *entry = path_join(appdir, path_suffixes[i]);
            path_prefix = append_colon_list(path_prefix, entry);
            free(entry);
        }
        set_prefixed_env("PATH", path_prefix, getenv("PATH"));
        free(path_prefix);
    }

    {
        char *value = path_join(appdir, "/usr/lib/x86_64-linux-gnu/gdk-pixbuf-2.0/2.10.0/loaders");
        set_env("GDK_PIXBUF_MODULEDIR", value);
        free(value);
    }
    {
        char *value = path_join(appdir, "/usr/lib/x86_64-linux-gnu/gdk-pixbuf-2.0/2.10.0/loaders.cache");
        set_env("GDK_PIXBUF_MODULE_FILE", value);
        free(value);
    }
    {
        char *value = path_join(appdir, "/usr/lib/x86_64-linux-gnu/gio/modules");
        set_env("GIO_MODULE_DIR", value);
        free(value);
    }
    {
        char *value = path_join(appdir, "/usr/share/glib-2.0/schemas");
        set_env("GSETTINGS_SCHEMA_DIR", value);
        free(value);
    }
    {
        char *value = path_join(appdir, "/usr/lib/x86_64-linux-gnu/gstreamer-1.0");
        set_env("GST_PLUGIN_PATH", value);
        set_env("GST_PLUGIN_SYSTEM_PATH", value);
        free(value);
    }
    {
        char *value = path_join(appdir, "/usr/lib/x86_64-linux-gnu/gstreamer1.0/gstreamer-1.0/gst-plugin-scanner");
        set_env("GST_PLUGIN_SCANNER", value);
        free(value);
    }
    {
        char *value = path_join(appdir, "/usr/lib/x86_64-linux-gnu/gstreamer1.0/gstreamer-1.0/gst-ptp-helper");
        set_env("GST_PTP_HELPER", value);
        free(value);
    }

    set_env("GST_REGISTRY_REUSE_PLUGIN_SCANNER", "no");
    {
        char *value = path_join(appdir, "/usr");
        set_env("GTK_EXE_PREFIX", value);
        set_env("GTK_DATA_PREFIX", value);
        free(value);
    }
    set_prefixed_env("LD_LIBRARY_PATH", libpaths, getenv("LD_LIBRARY_PATH"));
    unsetenv("LD_PRELOAD");
    maybe_detach_for_terminal_launch();
    set_env("GIO_LAUNCHED_DESKTOP_FILE", installed_desktop_file != NULL ? installed_desktop_file : desktop_file);
    set_numeric_env("GIO_LAUNCHED_DESKTOP_FILE_PID", (long) getpid());

    char **child_argv = calloc((size_t) argc + 1, sizeof(char *));
    if (child_argv == NULL) {
        die("AppRun: out of memory");
    }

    child_argv[0] = binary;
    for (int i = 1; i < argc; ++i) {
        child_argv[i] = argv[i];
    }
    child_argv[argc] = NULL;

    if (chdir(appdir) != 0) {
        die("AppRun: failed to chdir to %s", appdir);
    }

    execv(binary, child_argv);
    perror("AppRun: execv failed");
    free(child_argv);
    free(binary);
    free(installed_desktop_file);
    free(desktop_file);
    free(libpaths);
    free(appdir);
    return 1;
}
