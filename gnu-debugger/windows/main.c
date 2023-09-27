#include <stdio.h>
#include <windows.h>
#include <stdbool.h>

#define MCPU_MAX_LEN 16
#define MCPU_PREFIX "--mcpu="
#define MCPU_BASE "esp"

#if TARGET_ESP_ARCH_XTENSA
#define TARGET_ESP_ARCH_PREFIX "xtensa-"
#elif TARGET_ESP_ARCH_RISCV32
#define TARGET_ESP_ARCH_PREFIX "riscv32-"
#else
#error "Unknown TARGET_ESP_ARCH"
#endif

#define GDB_FILENAME_PREFIX TARGET_ESP_ARCH_PREFIX MCPU_BASE
#define GDB_BASE_FILENAME TARGET_ESP_ARCH_PREFIX MCPU_BASE "-elf-gdb.exe"
#define GDB_FILENAME_EXAMPLE TARGET_ESP_ARCH_PREFIX MCPU_BASE "XXX-elf-gdb.exe"
#define GDB_NO_PYTHON_SUFFIX "no-python"
#define GDB_EXTENSION ".exe"

#define GDB_ARG_BATCH_SILENT "--batch-silent"

#define PYTHON_SCRIPT_CMD_OPTION " -c "
#define PYTHON_SCRIPT_BODY "\"import sys;"\
"print(\\\"{}.{}\\\".format(sys.version_info.major, sys.version_info.minor));"\
"print(sys.base_prefix);\""

#define REDIRECT_STDERR_TO_NULL " 2>nul"
#define PYTHON_MAJOR_WITH_DOT "3."

#define PRINT_MESSAGE(...) \
do                         \
{                          \
  if (print_messages) {    \
    printf(__VA_ARGS__);   \
    fflush(stdout);        \
  }                        \
} while(0);


#if TARGET_ESP_ARCH_XTENSA
static void set_mcpu_option(const char *filename, char *mcpu, const size_t mcpu_size);
#endif
static char *get_module_filename(size_t append_memory_size);
static char *get_filename_ptr(const char *exe_path);
static char *get_exe_path_and_mcpu_option(const char *python_version, char *mcpu_option, const size_t mcpu_option_size);
static char *get_cmdline(const int argc, const char **argv, const char *exe_path, const char * mcpu_option);
static void get_python_info(char **version, char **base_prefix);
static int execute_cmdline(const char *cmdline, BOOL test_run);
static int update_environment_variables(const char *python_base_prefix);
static int run_gdb(const char *python_version, const int argc, const char ** argv, BOOL test_run);

const char *python_exe_arr[] = {"python", "python3"};

int print_messages = 0;

// Workflow:
// 1. Get python version and python base_prefix. (python executables to check are in python_exe_arr)
// 2. Set PYTHONHOME and append PATH environment variables with base_prefix from step 1
// 3. Find GDB binary with python version from step 1. (GDB without python used if errors on steps 1,2)
// 4. Execute GDB binary as a child process
// 5. Disable ctrl+c and ctrl+break for this wrapper process
// 6. Wait until GDB exit
int main (int argc, char **argv) {
  char *python_version;
  char *python_base_prefix;
  const char *trace_str = getenv ("ESP_DEBUG_TRACE");
  int exit_code = 0;
  if(trace_str) {
    print_messages = atoi(trace_str) > 0;
  }

  get_python_info(&python_version, &python_base_prefix);
  if (python_version && update_environment_variables(python_base_prefix)) {
    // start gdb without python if setting environment was failed
    PRINT_MESSAGE("update_environment_variables() failed, gdb without-python will be used\r\n");
    free(python_version);
    python_version = NULL;
  }

  if (python_base_prefix) {
    free(python_base_prefix);
  }

  if (python_version) {
    // run GDB with-python to check if it executes well
    char *test_argv[2] = { NULL, GDB_ARG_BATCH_SILENT };
    if (run_gdb(python_version, 2, (const char **) test_argv, TRUE)) {
      PRINT_MESSAGE("GDB with-python test execution failed, use no-python GDB\r\n");
      free(python_version);
      python_version = NULL;
    }
  }

  exit_code = run_gdb(python_version, (const int) argc, (const char **) argv, FALSE);

  if (python_version) {
    free(python_version);
  }
  return exit_code;
}

static int run_gdb(const char *python_version, const int argc, const char ** argv, BOOL test_run) {
  char *cmdline = NULL;
  char *exe_path = NULL;
  char mcpu_option[MCPU_MAX_LEN] = {0};
  int exit_code = 0;

  exe_path = get_exe_path_and_mcpu_option(python_version, mcpu_option, sizeof(mcpu_option));
  cmdline = get_cmdline(argc, argv, exe_path, mcpu_option);
  exit_code = execute_cmdline(cmdline, test_run);

  free(exe_path);
  free(cmdline);
  return exit_code;
}

char *readline(FILE *f)
{
  const size_t chunk_size = 1024;
  char *buf = calloc(1, chunk_size);
  char *pos = buf;
  size_t len = 0;
  char *tmp = NULL;

  if (!buf) {
    perror("calloc()");
    abort();
  }

  while (fgets(pos, chunk_size, f)) {
    char *nl = strchr(pos, '\n');
    if (nl) {
      // newline found, replace with string terminator
      *nl = '\0';
      return buf;
    }

    // no newline, increase buffer size
    len += strlen(pos);
    tmp = realloc(buf, len + chunk_size);
    if (!tmp) {
      free(buf);
      perror("realloc()");
      abort();
    }
    buf = tmp;
    pos = buf + len;
  }

  // handle case when input ends without a newline
  if (buf[0] == '\0') {
    free(buf);
    buf = NULL;
  }
  return buf;
}
static char *get_module_filename(size_t append_memory_size) {
  LPTSTR exe_path;
  DWORD exe_path_size;
  DWORD res;

  exe_path_size = PATH_MAX;
  exe_path = malloc(exe_path_size);
  if (exe_path == NULL) {
    perror("malloc()");
    abort();
  }

  // For some reasons it could be a path greater than PATH_MAX,
  // so allocate more memory until it fits to the buffer
  while ((res = GetModuleFileName(0, exe_path, exe_path_size)) &&
         (GetLastError() == ERROR_INSUFFICIENT_BUFFER)) {
    exe_path_size *= 2;
    exe_path = realloc(exe_path, exe_path_size);
    if (exe_path == NULL) {
      perror("realloc()");
      abort();
    }
  }

  if (!res) {
    perror("GetModuleFileName()");
    abort();
  }

  // resize buffer if need for future usage
  if (exe_path_size < strlen(exe_path) + append_memory_size + 1) {
    exe_path_size = strlen(exe_path) + append_memory_size + 1;
    exe_path = realloc(exe_path, exe_path_size);
    if (exe_path == NULL) {
      perror("realloc()");
      abort();
    }
  }
  return exe_path;
}

static char *get_filename_ptr(const char *exe_path) {
  char *filename = strrchr(exe_path, '\\');
  if(filename == NULL) {
    fprintf(stderr, "Wrong path, can't extract filename (\"%s\")", exe_path);
    abort();
  }

  if (strlen(filename) - 1 < strlen(GDB_BASE_FILENAME)) {
    fprintf(stderr, "Filename is too short. Expected \"%s\"", GDB_FILENAME_EXAMPLE);
    abort();
  }

  return filename++;
}

#if TARGET_ESP_ARCH_XTENSA
static void set_mcpu_option(const char *filename, char *mcpu, const size_t mcpu_option_size) {
  char *mcpu_start = strchr(filename, '-');
  char *mcpu_end = strchr(&filename[strlen(GDB_FILENAME_PREFIX)], '-');
  size_t len_to_write = 0;

  if (mcpu_start == NULL || mcpu_end == NULL) {
    fprintf(stderr, "Wrong filename format. Expected \"%s\"", GDB_FILENAME_EXAMPLE);
    abort();
  }
  mcpu_start++;

  len_to_write = mcpu_end - mcpu_start + strlen(MCPU_PREFIX);
  if (mcpu_option_size < len_to_write) {
    fprintf(stderr, "insufficient buffer size\n");
    abort();
  }
  snprintf(mcpu, len_to_write, "%s%s", MCPU_PREFIX, mcpu_start);

  return;
}
#endif

static char *get_exe_path_and_mcpu_option(const char *python_version, char *mcpu_option, const size_t mcpu_option_size) {
  char *exe_path = NULL;
  char *base_path = NULL;
  char *filename = NULL;
  const char *python_suffix = python_version ? python_version : GDB_NO_PYTHON_SUFFIX;
  size_t chars_to_remove = 0;
  char *start = NULL;
  size_t chars_to_move = 0;

  exe_path = get_module_filename(strlen(python_suffix) + 1);
  filename = get_filename_ptr(exe_path);

#if TARGET_ESP_ARCH_XTENSA
  set_mcpu_option(filename, mcpu_option, mcpu_option_size);

  // Remove esp mcpu from filename
  chars_to_remove = strlen(mcpu_option) - strlen(MCPU_PREFIX) - strlen(MCPU_BASE);
  start = filename + strlen(GDB_FILENAME_PREFIX) + 1;
  chars_to_move = strlen(start) - chars_to_remove + 1;
  memmove(start, start + chars_to_remove, chars_to_move);
#endif

  // insert python_version to the filename
  // don't worry about buffer overflow, additionall memory for python version
  // was allocated in get_module_filename()
  base_path = strdup(exe_path);
  if (base_path == NULL) {
    perror("strdup()");
    abort();
  }
  base_path[strlen(base_path) - strlen(GDB_EXTENSION)] = '\0';

  sprintf(exe_path, "%s-%s%s", base_path, python_suffix, GDB_EXTENSION);
  if(python_suffix != GDB_NO_PYTHON_SUFFIX) {
    if (GetFileAttributesA(exe_path) == INVALID_FILE_ATTRIBUTES) { // no exe file for this python version
      sprintf(exe_path, "%s-%s%s", base_path, GDB_NO_PYTHON_SUFFIX, GDB_EXTENSION);
      PRINT_MESSAGE("Python-%s is not supported. Run without python\r\n", python_version);
    } else {
      PRINT_MESSAGE("Run with python-%s\r\n", python_version);
    }
  } else {
    PRINT_MESSAGE("Run without python\r\n");
  }

  free(base_path);
  return exe_path;
}

static char *get_cmdline(const int argc, const char **argv, const char *exe_path, const char * mcpu_option) {
  char * cmdline = (char *)malloc(strlen(exe_path) + strlen(mcpu_option) + 2);
  if (cmdline == NULL) {
    perror("malloc");
    abort();
  }
  sprintf(cmdline, "%s %s", exe_path, mcpu_option);

  // Append with user's arguments. Protect them with quotes
  for (int i = 1; i < argc; i++) {
    size_t cur_len = strlen(cmdline);
    cmdline = realloc(cmdline, cur_len + strlen(argv[i]) + 4);
    if (cmdline == NULL) {
      perror("realloc");
      abort();
    }
    sprintf(&cmdline[cur_len], " \"%s\"", argv[i]);
  }
  return cmdline;
}

static int update_environment_variables(const char *python_base_prefix) {
  int ret = -1;
  DWORD path_var_size = 0;
  char *buf = NULL;
  int len = 0;

  if (!python_base_prefix) {
    PRINT_MESSAGE("%s: python_base_prefix is NULL\r\n", __FUNCTION__);
    goto error;
  }

  // If buffer is not large enough, the return value is the number of
  // characters including the terminating null character.
  path_var_size = GetEnvironmentVariable("PATH", NULL, 0); // get size of PATH variable

  // alloc memory for existing PATH + python_base_prefix + ';' delimeter + '\0'
  buf = (char *)malloc(path_var_size + strlen(python_base_prefix) + 2);
  if (!buf) {
    perror("malloc()");
    abort();
  }

  // start PATH with directory contains python*.dll
  len = sprintf(buf, "%s;", python_base_prefix);
  if (len < strlen(python_base_prefix) + 1) {
    PRINT_MESSAGE("Error writing python_base_prefix '%s' to buf (%d)\r\n", python_base_prefix, len);
    goto error;
  }

  // If the function succeeds, the return value is the number of characters
  // NOT including the terminating null character.
  if ((path_var_size - 1) != GetEnvironmentVariable("PATH", &buf[len], path_var_size)) {
    PRINT_MESSAGE("GetEnvironmentVariable() failed to get. PATH length changed??\r\n");
    goto error;
  }
  if (!SetEnvironmentVariable("PATH", buf)) {
    PRINT_MESSAGE("SetEnvironmentVariable(PATH) failed: %s\r\n", GetLastError());
    goto error;
  }

  // Set PYTHONHOME to have base python modules
  if (!SetEnvironmentVariable("PYTHONHOME", python_base_prefix)) {
    PRINT_MESSAGE("SetEnvironmentVariable(PYTHONHOME) failed: %s\r\n", GetLastError());
    goto error;
  }

  ret = 0;
error:
  if (buf) {
    free(buf);
  }
  return ret;
}

static void get_python_info(char **version, char **base_prefix) {
  char *tmp = NULL;
  const size_t python_exe_arr_size = sizeof(python_exe_arr) / sizeof(python_exe_arr[0]);
  size_t i = 0;

  if (version == NULL || base_prefix == NULL) {
    fprintf(stderr, "%s: bad input parameters (%p %p)\n", __FUNCTION__, version, base_prefix);
    abort();
  }

  *version = NULL;
  *base_prefix = NULL;

  for (i = 0; i < python_exe_arr_size; i++) {
    FILE* pipe = NULL;
    char *python_cmd = malloc(strlen(python_exe_arr[i]) +
                              strlen(PYTHON_SCRIPT_CMD_OPTION) +
                              strlen(PYTHON_SCRIPT_BODY) +
                              strlen(REDIRECT_STDERR_TO_NULL) +
                              1);
    sprintf(python_cmd, "%s%s%s%s", python_exe_arr[i],
            PYTHON_SCRIPT_CMD_OPTION,
            PYTHON_SCRIPT_BODY,
            REDIRECT_STDERR_TO_NULL);

    pipe = popen(python_cmd, "r");
    free(python_cmd);

    if (pipe == NULL) {
      continue;
    }

    *version = readline(pipe);
    if (*version == NULL) {
      pclose(pipe);
      continue;
    }

    PRINT_MESSAGE("Found python version: %s\r\n", *version);
    if (strncmp(*version, PYTHON_MAJOR_WITH_DOT, strlen(PYTHON_MAJOR_WITH_DOT)) == 0) {
      *base_prefix = readline(pipe);
      pclose(pipe);
      break;
    }

    free(*version);
    *version = NULL;
    pclose(pipe);
  }
}

static int execute_cmdline(const char *cmdline, BOOL test_run) {
  STARTUPINFO si;
  PROCESS_INFORMATION pi;
  DWORD exit_code = 0;

  PRINT_MESSAGE("Executing: \"%s\"\r\n", cmdline);

  ZeroMemory(&si, sizeof(si));
  si.cb = sizeof(si);
  ZeroMemory(&pi, sizeof(pi));

  // Do not show popup boxes with errors on test run
  SetErrorMode(test_run ? (SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX) : 0);

  if (!CreateProcessA(NULL,                // No module name (use command line)
                      (char *) cmdline,    // Command line
                      NULL,                // Process handle not inheritable
                      NULL,                // Thread handle not inheritable
                      TRUE,                // Set handle inheritance to TRUE
                      0,                   // No creation flags
                      NULL,                // Use parent's environment block
                      NULL,                // Use parent's starting directory
                      &si,                 // Pointer to STARTUPINFO structure
                      &pi)                 // Pointer to PROCESS_INFORMATION structure
     ) {
    fprintf(stderr, "Can't execute \"%s\"\nError: %s", cmdline, GetLastError());
    abort();
  }

  if (!test_run) {
    // Disable ctrl+c handling for this GDB wrapper
    // Setting this on test-run breaks Ctrl handlings on target GDB
    SetConsoleCtrlHandler(NULL, TRUE);
  }

  // Wait until child process exits.
  WaitForSingleObject(pi.hProcess, INFINITE);

  // Get exit code of child process
  GetExitCodeProcess(pi.hProcess, &exit_code);
  PRINT_MESSAGE("Exit code is %d\r\n", exit_code);

  // Close process and thread handles.
  CloseHandle(pi.hProcess);
  CloseHandle(pi.hThread);

  return (int) exit_code;
}
