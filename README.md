# **The TaaJ OS**
The TaaJ OS is an open-source operating system developed by [The Jaat](https://github.com/TheJaat). This repository serves as the core OS repository, integrating components such as the bootloader and kernel. It aims to provide a modular, lightweight, and efficient environment for learning and experimentation in systems programming.

## **Repository Structure**
This repository includes the following submodules:
- **[Bootloader](https://github.com/TheJaat/TheBootloader):** Responsible for initializing the system and loading the kernel.
- **[Kernel](https://github.com/TheJaat/TheKernel):** Contains the core logic and drivers of the operating system.


## **Features**
- Modular bootloader and kernel design.
- Support for 32-bit.
- Extensible configuration through a kernel loading mechanism.


## **Getting Started**
Follow these steps to set up and build the OS:

1. **Clone the Repository:**
   Clone this repository and initialize submodules:
   ```bash
   git clone --recurse-submodules git@github.com:TheJaat/TheTaaJ.git
   cd TheTaaJ
   ```
2. **Build the Toolchain:**
   Navigate to the toolchain directory and follow the instructions provided in the `README.md` file to build the toolchain.
3. **Build the OS:**
   Use the provided `Makefile` to build the bootloader and kernel:
   ```bash
   make
   ```
4. **Run the OS:**
   After building, you can test the OS using an emulator like QEMU:
   ```bash
   make run
   ```

## Contribution Guidelines
We welcome contributions! To contribute:
1. Fork this repository and clone your fork.
2. Create a new branch for your feature or bugfix.
3. Submit a pull request with a detailed description.

Please ensure your changes align with the project's coding standards and include appropriate documentation.
