package main

import (
	"bufio"
	"fmt"
	"log"
	"os"
	"path/filepath"
	"regexp"
	"strings"

	"github.com/spf13/cobra"
)

var commandPackage = &cobra.Command{
	Use:   "package",
	Short: "Copy built libraries and C headers into lib/ and include/",
	Run: func(cmd *cobra.Command, args []string) {
		targets := parseTargets()
		packageTargets(targets)
	},
}

func init() {
	mainCommand.AddCommand(commandPackage)
}

func packageTargets(targets []Target) {
	log.Printf("Packaging libraries for %d target(s)", len(targets))

	libraryDirectory := filepath.Join(projectRoot, "lib")
	includeDirectory := filepath.Join(projectRoot, "include")

	os.RemoveAll(libraryDirectory)
	os.RemoveAll(includeDirectory)
	os.MkdirAll(includeDirectory, 0o755)

	headers := []struct {
		source      string
		destination string
	}{
		{filepath.Join(srcRoot, "components/cronet/native/include/cronet_c.h"), "cronet_c.h"},
		{filepath.Join(srcRoot, "components/cronet/native/include/cronet_export.h"), "cronet_export.h"},
		{filepath.Join(srcRoot, "components/cronet/native/generated/cronet.idl_c.h"), "cronet.idl_c.h"},
		{filepath.Join(srcRoot, "components/grpc_support/include/bidirectional_stream_c.h"), "bidirectional_stream_c.h"},
	}

	for _, header := range headers {
		copyFile(header.source, filepath.Join(includeDirectory, header.destination))
	}
	log.Print("Copied headers to include/")

	for _, t := range targets {
		directoryName := getLibraryDirectoryName(t)
		targetDirectory := filepath.Join(libraryDirectory, directoryName)
		os.MkdirAll(targetDirectory, 0o755)

		outputDirectory := getOutputDirectory(t)

		if t.GOOS == "windows" {
			// Windows: only a DLL is produced (static linking not supported -
			// Chromium uses MSVC, no MinGW static lib).
			sourceDLL := filepath.Join(srcRoot, outputDirectory, "cronet.dll")
			destinationDLL := filepath.Join(targetDirectory, "libcronet.dll")
			if _, err := os.Stat(sourceDLL); os.IsNotExist(err) {
				log.Printf("Warning: DLL not found for %s/%s, skipping", t.GOOS, t.ARCH)
			} else {
				copyFile(sourceDLL, destinationDLL)
				log.Printf("Copied DLL for %s/%s", t.GOOS, t.ARCH)
			}
			log.Printf("Packaged lib/%s", directoryName)
			continue
		}

		sourceStatic := filepath.Join(srcRoot, outputDirectory, "obj/components/cronet/libcronet_static.a")
		destinationStatic := filepath.Join(targetDirectory, "libcronet.a")
		if _, err := os.Stat(sourceStatic); os.IsNotExist(err) {
			log.Printf("Warning: static library not found for %s, skipping", formatTargetLog(t))
		} else {
			copyFile(sourceStatic, destinationStatic)
			log.Printf("Copied static library for %s", formatTargetLog(t))
		}

		// For Linux glibc, also copy the shared library.
		if t.GOOS == "linux" && t.Libc != "musl" {
			sourceShared := filepath.Join(srcRoot, outputDirectory, "libcronet.so")
			destinationShared := filepath.Join(targetDirectory, "libcronet.so")
			if _, err := os.Stat(sourceShared); err == nil {
				copyFile(sourceShared, destinationShared)
				log.Printf("Copied shared library for %s", formatTargetLog(t))
			}
		}

		// Dump the system link flags so the static library can actually be
		// linked by a downstream C/C++ project.
		writeLinkFlags(t, targetDirectory, outputDirectory)

		log.Printf("Packaged lib/%s", directoryName)
	}

	log.Print("Package complete!")
}

// writeLinkFlags extracts the libs/frameworks/ldflags that the static library
// depends on (parsed from the generated ninja file) and writes them to a plain
// text file next to the library.
func writeLinkFlags(t Target, targetDirectory, outputDirectory string) {
	flags, err := extractLinkFlags(outputDirectory)
	if err != nil {
		log.Printf("Warning: could not extract link flags for %s: %v", formatTargetLog(t), err)
		return
	}

	var b strings.Builder
	if len(flags.LDFlags) > 0 {
		fmt.Fprintf(&b, "# ldflags\n%s\n", strings.Join(flags.LDFlags, " "))
	}
	if len(flags.Libs) > 0 {
		fmt.Fprintf(&b, "# libs\n%s\n", strings.Join(flags.Libs, " "))
	}
	if len(flags.Frameworks) > 0 {
		fmt.Fprintf(&b, "# frameworks\n%s\n", strings.Join(flags.Frameworks, " "))
	}
	if t.GOOS == "linux" && t.Libc == "musl" {
		b.WriteString("# extra\n-static\n")
	}
	if b.Len() == 0 {
		return
	}

	path := filepath.Join(targetDirectory, "link_flags.txt")
	if err := os.WriteFile(path, []byte(b.String()), 0o644); err != nil {
		log.Printf("Warning: failed to write link_flags.txt for %s: %v", formatTargetLog(t), err)
		return
	}
	log.Printf("Wrote link flags for %s", formatTargetLog(t))
}

func getLibraryDirectoryName(t Target) string {
	osName := t.GOOS
	if t.Platform == "tvos" {
		osName = "tvos"
	}

	name := fmt.Sprintf("%s_%s", osName, t.ARCH)

	if t.Environment == "simulator" {
		name += "_simulator"
	}

	if t.Libc == "musl" {
		name += "_musl"
	}

	return name
}

type LinkFlags struct {
	Libs       []string
	Frameworks []string
	LDFlags    []string
}

func extractLinkFlags(outputDirectory string) (LinkFlags, error) {
	ninjaPath := filepath.Join(srcRoot, outputDirectory, "obj/components/cronet/cronet_sample.ninja")
	file, err := os.Open(ninjaPath)
	if err != nil {
		return LinkFlags{}, fmt.Errorf("failed to open ninja file %s: %w", ninjaPath, err)
	}
	defer file.Close()

	var flags LinkFlags
	libsRegex := regexp.MustCompile(`^\s*libs\s*=\s*(.*)$`)
	frameworksRegex := regexp.MustCompile(`^\s*frameworks\s*=\s*(.*)$`)
	ldflagsRegex := regexp.MustCompile(`^\s*ldflags\s*=\s*(.*)$`)

	scanner := bufio.NewScanner(file)
	for scanner.Scan() {
		line := scanner.Text()

		if matches := libsRegex.FindStringSubmatch(line); matches != nil {
			libsStr := strings.TrimSpace(matches[1])
			if libsStr != "" {
				for _, lib := range strings.Fields(libsStr) {
					// Filter out linker scripts (.lds) - not needed for static linking.
					if !strings.HasSuffix(lib, ".lds") {
						flags.Libs = append(flags.Libs, lib)
					}
				}
			}
		}

		if matches := frameworksRegex.FindStringSubmatch(line); matches != nil {
			frameworksStr := strings.TrimSpace(matches[1])
			if frameworksStr != "" {
				flags.Frameworks = parseFrameworks(frameworksStr)
			}
		}

		if matches := ldflagsRegex.FindStringSubmatch(line); matches != nil {
			ldflagsStr := strings.TrimSpace(matches[1])
			if ldflagsStr != "" {
				flags.LDFlags = parseLDFlags(ldflagsStr)
			}
		}
	}

	if err := scanner.Err(); err != nil {
		return LinkFlags{}, fmt.Errorf("failed to read ninja file: %w", err)
	}

	return flags, nil
}

func parseFrameworks(input string) []string {
	var result []string
	parts := strings.Fields(input)
	for i := 0; i < len(parts); i++ {
		if parts[i] == "-framework" && i+1 < len(parts) {
			result = append(result, "-framework "+parts[i+1])
			i++
		}
	}
	return result
}

func parseLDFlags(input string) []string {
	var result []string
	for _, flag := range strings.Fields(input) {
		// Extract -Wl,-wrap,* flags needed for Android allocator shim.
		if strings.HasPrefix(flag, "-Wl,-wrap,") {
			result = append(result, flag)
		}
	}
	return result
}
