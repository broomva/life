# Harness engineering targets (loaded first, can be overridden)
-include Makefile.harness

# Control metalayer targets (loaded last, takes precedence for overlapping targets)
-include Makefile.control
