#!/usr/bin/env python3
import yaml
import json
import pymodbus.client
import argparse
from pathlib import Path
import logging
from typing import Dict, Any, List, Tuple, Optional
import time
import sys
import socket
import struct
import select

# Configure logging
logging.basicConfig(
    level=logging.INFO,
    format='%(asctime)s - %(levelname)s - %(message)s'
)
logger = logging.getLogger(__name__)

class CustomModbusClient:
    """Custom Modbus client that handles EG4 protocol specifics."""
    def __init__(self, host, port=8000, connect_timeout=5.0, read_timeout=30.0, delay_ms=200):
        self.host = host
        self.port = port
        self.connect_timeout = connect_timeout
        self.read_timeout = read_timeout
        self.delay_ms = delay_ms
        self.sock = None
        self.datalog_id = bytearray(10)  # Initialize with zeros
        self.inverter_serial = bytearray(10)  # Initialize with zeros
        self.last_heartbeat = 0
        self.logger = logging.getLogger(__name__)
        self.max_buffer_size = 1024
        self.reconnect_delay = 5.0  # 5 seconds reconnect delay
        self.tcp_keepalive = 60  # 60 seconds TCP keepalive
        self.max_buffer_size = 65536  # 64KB max buffer size (matching Rust)

    def connect(self):
        """Connect to the inverter with proper socket options."""
        try:
            self.sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            self.sock.settimeout(self.connect_timeout)  # Set timeout for connection
            self.sock.connect((self.host, self.port))
            
            # Set TCP_NODELAY for lower latency
            self.sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
            
            # Set TCP keepalive
            self.sock.setsockopt(socket.SOL_SOCKET, socket.SO_KEEPALIVE, 1)
            self.sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_KEEPIDLE, self.tcp_keepalive)
            self.sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_KEEPINTVL, self.tcp_keepalive)
            self.sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_KEEPCNT, 3)
            
            self.logger.info(f"Connection successful on port {self.port}")
            self.last_heartbeat = time.time()
            return True
        except Exception as e:
            self.logger.error(f"Failed to connect: {e}")
            self.sock = None
            return False

    def calculate_crc16(self, data):
        """Calculate CRC16 for the given data."""
        crc = 0xFFFF
        for b in data:
            crc ^= b
            for _ in range(8):
                if crc & 0x0001:
                    crc = (crc >> 1) ^ 0xA001
                else:
                    crc = crc >> 1
        # Return CRC in little-endian byte order
        return bytes([crc & 0xFF, (crc >> 8) & 0xFF])

    def build_packet(self, register_address, count):
        """Build a packet to read registers."""
        # Build data section
        data = bytearray()
        data.append(0x00)  # Source (0=client)
        data.append(0x03)  # Function code (3=read holding registers)
        # CRITICAL_SECTION_DO_NOT_MODIFY_OR_REMOVE: Must use most recently received inverter serial
        # SYSTEM_CRITICAL: Connection maintenance
        data.extend(self.inverter_serial)  # Inverter serial (10 bytes)
        self.logger.info(f"Transmitting with inverter serial: {self.inverter_serial}")
        # END_CRITICAL_SECTION_SERIAL
        
        # Convert register address and count to bytes, ensuring they're in valid range
        data.extend((register_address & 0xFFFF).to_bytes(2, byteorder='little'))  # Register address
        data.extend((count & 0xFFFF).to_bytes(2, byteorder='little'))  # Register count
        
        # Calculate data section length
        data_len = len(data)
        self.logger.debug(f"Data section length: {data_len}")
        self.logger.debug(f"Data section: {data.hex()}")
        
        # Build header
        header = bytearray()
        header.extend(b'\xa1\x1a')  # Magic bytes
        header.extend((0x01).to_bytes(2, byteorder='little'))  # Protocol version (1 for read)
        header.extend((data_len + 12).to_bytes(2, byteorder='little'))  # Frame length (data_len + 12 for header)
        header.append(0x01)  # Unknown, always 1
        header.append(0xc2)  # TCP function (0xc2=read holding registers)
        # CRITICAL_SECTION_DO_NOT_MODIFY_OR_REMOVE: Must use most recently received datalog ID
        # SYSTEM_CRITICAL: Connection maintenance
        header.extend(self.datalog_id)  # Datalog ID
        self.logger.info(f"Transmitting with datalog ID: {self.datalog_id}")
        # END_CRITICAL_SECTION_DATALOG
        
        self.logger.debug(f"Header length: {len(header)}")
        self.logger.debug(f"Header: {header.hex()}")
        
        # CRITICAL_SECTION_DO_NOT_MODIFY_OR_REMOVE: CRC calculation and verification
        # SYSTEM_CRITICAL: Protocol compliance
        crc = self.calculate_crc16(header + data)
        self.logger.debug(f"Calculated CRC: {crc.hex()}")
        
        # Combine all parts
        packet = header + data + crc
        self.logger.debug(f"Total packet length: {len(packet)}")
        self.logger.debug(f"Complete packet: {packet.hex()}")
        
        # Verify the CRC we just calculated
        if crc == packet[-2:]:
            self.logger.info("Outgoing packet CRC check passed ✓")
        else:
            self.logger.warning(f"Outgoing packet CRC mismatch! Calculated: {crc.hex()}, Packet: {packet[-2:].hex()}")
        # END_CRITICAL_SECTION_CRC
        
        return packet

    def build_heartbeat_packet(self):
        """Build a heartbeat packet."""
        # Build header
        header = bytearray()
        header.extend(b'\xa1\x1a')  # Magic bytes
        header.extend((2).to_bytes(2, byteorder='little'))  # Protocol version 2
        header.extend((18).to_bytes(2, byteorder='little'))  # Frame length (header only)
        header.extend([1])  # Unknown
        header.extend([193])  # TCP function (Heartbeat)
        # CRITICAL_SECTION_DO_NOT_MODIFY_OR_REMOVE: Must use most recently received datalog ID
        # SYSTEM_CRITICAL: Connection maintenance
        header.extend(self.datalog_id)  # Datalog ID
        self.logger.info(f"Heartbeat - Transmitting with datalog ID: {self.datalog_id.decode()}")
        # END_CRITICAL_SECTION_DATALOG
        
        # Heartbeat has no data section, just a zero byte
        data = bytearray([0])
        
        # No CRC for heartbeat packets
        packet = header + data
        self.logger.debug(f"Built heartbeat packet: {packet.hex()}")
        return packet

    def send_heartbeat(self):
        """Send a heartbeat packet to keep the connection alive."""
        try:
            packet = self.build_heartbeat_packet()
            self.sock.sendall(packet)
            self.last_heartbeat = time.time()
            self.logger.debug("Sent heartbeat packet")
            return True
        except Exception as e:
            self.logger.error(f"Failed to send heartbeat: {e}")
            return False

    def check_and_send_heartbeat(self):
        """Check if it's time to send a heartbeat and send if needed."""
        # Send heartbeat every 30 seconds
        if time.time() - self.last_heartbeat >= 30:
            return self.send_heartbeat()
        return True

    def read_response(self):
        """Read and parse the response from the inverter."""
        try:
            # Set a timeout for the read operation
            self.sock.settimeout(30)  # 30 second timeout
            
            # Read all available data in chunks until we have the complete response
            data = bytearray()
            while True:
                chunk = self.sock.recv(1024)
                if not chunk:
                    break
                data.extend(chunk)
                
                # Check if we have a complete packet
                if len(data) >= 19:  # Minimum header length
                    # Check for heartbeat response
                    if data[0:4] == b'\xa1\x1a\x02\x00':
                        self.logger.debug("Received heartbeat response")
                        return None
                        
                    # Check if we have the complete packet
                    if len(data) >= 21:  # Header + data + CRC
                        break
                        
            if not data:
                raise Exception("No data received")
                
            if len(data) < 21:  # Minimum packet size
                raise Exception(f"Response too short: {len(data)} bytes")
                
            # Split into header, data section, and CRC
            header = data[:19]
            data_section = data[19:-2]
            received_crc = data[-2:]
            
            self.logger.debug(f"Received header: {header.hex()} - {header[8:18]}")
            self.logger.debug(f"Received data: {data_section.hex()} - {data_section[3:13]}")
            self.logger.debug(f"Received CRC: {received_crc.hex()}")
            
            # Calculate CRC over data section only (excluding header)
            calculated_crc = self.calculate_crc16(data_section)
            self.logger.debug(f"Calculated CRC: {calculated_crc.hex()}")
            
            # Compare CRCs properly (both are bytes objects)
            if received_crc != calculated_crc:
                self.logger.warning(f"CRC mismatch! Received: {received_crc.hex()}, Calculated: {calculated_crc.hex()}")
            
            # CRITICAL_SECTION_DO_NOT_MODIFY_OR_REMOVE_BEGIN
            # SYSTEM_CRITICAL: Protocol State Maintenance
            # These values MUST be preserved across all packets for proper protocol operation
            try:
                # Extract and preserve datalog ID from header (bytes 8-17)
                datalog_id = header[8:18] # CURSOR DO NOT TOUCH
                # Extract and preserve inverter serial from data section (bytes 2-11)
                inverter_serial = data_section[3:13] # CURSOR DO NOT TOUCH
                # Always log both old and new values
                self.logger.info(f"Previous datalog ID: {self.datalog_id.decode()}")
                self.logger.info(f"Received datalog ID: {datalog_id.decode()}")
                # Always update the value
                self.datalog_id = datalog_id
                if self.inverter_serial != inverter_serial:
                    self.logger.info(f"read_response:Previous serial ID (ascii): {self.inverter_serial.decode()} to {inverter_serial.decode()}")
                # Log both old and new values with hex representation
                # IMMUTABLE CODE FOR CURSOR DO NOT TOUCH
                self.datalog_id = datalog_id  # Create a new bytearray to ensure we have a copy
                self.inverter_serial = inverter_serial
                
                # Update config data with received IDs if it exists
                if 'config_data' in locals():
                    config_data['datalog_id'] = datalog_id
                    config_data['inverter_serial'] = inverter_serial
            except Exception as e:
                self.logger.error(f"Error extracting datalog ID or serial: {str(e)}")
                raise
            # END_CRITICAL_SECTION_DO_NOT_MODIFY_OR_REMOVE
            
            return data_section
            
        except Exception as e:
            self.logger.error(f"Error reading response: {str(e)}")
            raise

    def read_holding_registers(self, start_reg, count):
        """Read holding registers from the inverter."""
        max_retries = 3
        retry_count = 0
        
        while retry_count < max_retries:
            try:
                # Build and send the request packet
                packet = self.build_packet(start_reg, count)
                self.sock.sendall(packet)
                
                # Read the response
                response = self.read_response()
                if response is None:  # Heartbeat response
                    continue
                    
                # Process the response
                if len(response) < 2:  # Need at least 2 bytes for a register value
                    raise Exception(f"Response too short: {len(response)} bytes")
                    
                # Extract register values - each register is 2 bytes in little-endian order
                values = []
                try:
                    # Each register is 2 bytes in little-endian order
                    num_registers = len(response) // 2
                    for i in range(num_registers):
                        if i * 2 + 1 < len(response):
                            value = int.from_bytes(response[i*2:i*2+2], byteorder='little')
                            self.logger.debug(f"Register {start_reg + i}: {value} (0x{value:04x})")
                            values.append(value)
                except Exception as e:
                    self.logger.error(f"Error extracting register values: {str(e)}")
                    raise
                    
                # Add delay between reads
                time.sleep(self.delay_ms / 1000)  # Convert ms to seconds
                
                return values
                
            except Exception as e:
                retry_count += 1
                self.logger.warning(f"Attempt {retry_count} failed: {str(e)}")
                
                if retry_count < max_retries:
                    self.logger.info(f"Retrying in {self.delay_ms/1000} seconds...")
                    time.sleep(self.delay_ms / 1000)
                    # Try to reconnect if needed
                    if self.sock is None or not hasattr(self.sock, 'settimeout'):
                        self.connect()
                else:
                    self.logger.error(f"Failed after {max_retries} attempts")
                    raise Exception(f"Error reading from port {self.port}: {str(e)}")

    def close(self):
        """Close the connection."""
        if self.sock:
            try:
                self.sock.close()
            except:
                pass
            self.sock = None

    def try_connect_and_read(self, register_address, count):
        """Try to connect and read registers."""
        if not self.connect():
            return None
        try:
            return self.read_holding_registers(register_address, count)
        finally:
            self.close()

def test_connection(host: str, port: int) -> bool:
    """Test if we can connect to the host:port and read registers using the EG4 protocol."""
    try:
        client = CustomModbusClient(host, port, timeout=10)
        if not client.connect():
            return False
            
        # Try to read register 0
        logger.debug("Attempting to read register 0")
        result = client.read_holding_registers(0, 1)
        if result is not None:
            logger.info(f"Successfully read register 0: {result}")
            return True
            
        logger.error("Failed to read register 0")
        return False
        
    except Exception as e:
        logger.error(f"Connection test failed: {e}")
        return False
    finally:
        client.close()

def load_config(config_path: str) -> Dict[str, Any]:
    """Load configuration from YAML file."""
    try:
        with open(config_path, 'r') as f:
            return yaml.safe_load(f)
    except Exception as e:
        logger.error(f"Error loading config file {config_path}: {e}")
        sys.exit(1)

def load_registers(register_path: str) -> Dict[str, Any]:
    """Load register definitions from JSON file."""
    try:
        with open(register_path, 'r') as f:
            return json.load(f)
    except Exception as e:
        logger.error(f"Error loading register file {register_path}: {e}")
        sys.exit(1)

def decode_register_value(register_def: Dict[str, Any], value: int) -> Any:
    """Decode a register value based on its definition."""
    if register_def.get('display_as') == 'flags':
        flags = register_def.get('flags', [])
        result = {}
        for flag in flags:
            bit = flag['bit']
            result[flag['name']] = bool(value & (1 << bit))
        return result
    elif register_def.get('display_as') == 'enum':
        enum_map = register_def.get('enum_map', {})
        return enum_map.get(str(value), value)
    elif register_def.get('display_as') == 'float':
        # Convert to float and apply scaling
        scaling = float(register_def.get('scaling', 1.0))
        return float(value) * scaling
    else:
        # Default to integer value
        return value

def get_register_definition(registers: Dict[str, Any], register_type: str, register_number: int) -> Dict[str, Any]:
    """Get register definition from register map."""
    for reg_map in registers['registers']:
        if reg_map['register_type'] == register_type:
            for reg in reg_map['register_map']:
                if reg['register_number'] == register_number:
                    return reg
    return None

def create_modbus_client(host: str, port: int) -> pymodbus.client.ModbusTcpClient:
    """Create a Modbus TCP client with appropriate settings."""
    logger.debug(f"Creating Modbus client for {host}:{port}")
    client = pymodbus.client.ModbusTcpClient(
        host=host,
        port=port,
        timeout=5,  # 5 second timeout (matching Rust WRITE_TIMEOUT_SECS)
        retries=3,  # Increased retries for reliability
        retry_on_empty=True,
        close_comm_on_error=True,
        strict=False,
        tcp_nodelay=True,  # Enable TCP_NODELAY for lower latency
        tcp_keepalive=True,  # Enable TCP keepalive
        tcp_keepalive_interval=60  # 60 second keepalive interval
    )
    logger.debug("Modbus client created with settings:")
    logger.debug(f"  Timeout: 5s")
    logger.debug(f"  Retries: 3")
    logger.debug(f"  TCP_NODELAY: True")
    logger.debug(f"  TCP_KEEPALIVE: True")
    logger.debug(f"  TCP_KEEPALIVE_INTERVAL: 60s")
    return client

def read_inverter_config(client: CustomModbusClient, registers: Dict[str, Any]) -> Dict[str, Any]:
    """Read and decode all hold registers from inverter."""
    config = {}
    
    try:
        # Read initial register to get all values
        logger.debug("Reading initial register to get all values")
        response = client.read_holding_registers(0, 1)
        if not response:
            raise Exception("No response received for initial register read")
        
        logger.info(f"Received {len(response)} bytes of register data")
        
        # Process each register in the response
        for i in range(0, len(response), 2):
            reg_num = i // 2
            if reg_num >= 21:  # Don't process beyond register 20
                break
                
            # Read 2 bytes as little-endian value
            value = int.from_bytes(response[i:i+2], byteorder='little')
            reg_def = get_register_definition(registers, 'hold', reg_num)
            
            if reg_def:
                decoded_value = decode_register_value(reg_def, value)
                config[reg_def['shortname']] = {
                    'name': reg_def['name'],
                    'value': decoded_value,
                    'unit': reg_def.get('unit', ''),
                    'description': reg_def['description']
                }
                logger.info(f"Register {reg_num:2d}: {reg_def['shortname']:<30} = {value:5d} (0x{value:04x}) -> {decoded_value}")
            else:
                logger.warning(f"Register {reg_num:2d}: Unknown register, raw value = {value:5d} (0x{value:04x})")
            
    except Exception as e:
        logger.error(f"Error reading inverter config: {str(e)}")
        raise
        
    logger.info(f"Successfully read {len(config)} registers")
    return config

def try_connect_and_read(host: str, port: int, registers: Dict[str, Any]) -> Dict[str, Any]:
    """Try to connect to the inverter and read configuration."""
    client = None
    try:
        client = CustomModbusClient(host, port)
        if not client.connect():
            logger.error(f"Failed to establish connection to {host}:{port}")
            return None
            
        logger.info(f"Connection successful on port {port}, reading configuration...")
        
        # Read initial register to establish communication and get first batch of values
        response = client.read_holding_registers(0, 1)
        if not response:
            logger.error("Failed to read initial register")
            return None
            
        # If we got a different datalog/serial, log it
        if client.datalog_id:
            logger.info(f"Using received datalog ID: {client.datalog_id.decode()}")
        if client.inverter_serial:
            logger.info(f"Using received inverter serial: {client.inverter_serial.decode()}")
            
        # Process register values from the response
        config_data = {}
        
        # Process first batch of registers (0-20)
        for i in range(0, len(response), 2):
            reg_num = i // 2
            if reg_num >= 21:  # Don't process beyond register 20
                break
                
            # Read 2 bytes as little-endian value
            value = response[i] | (response[i+1] << 8)
            reg_def = get_register_definition(registers, 'hold', reg_num)
            
            if reg_def:
                decoded_value = decode_register_value(reg_def, value)
                config_data[reg_def['shortname']] = {
                    'name': reg_def['name'],
                    'value': decoded_value,
                    'unit': reg_def.get('unit', ''),
                    'description': reg_def['description']
                }
                logger.info(f"Register {reg_num:5d}: {reg_def['shortname']:<30} = {value:5d} (0x{value:04x}) -> {decoded_value}")
            else:
                logger.warning(f"Register {reg_num:5d}: Unknown register, raw value = {value:5d} (0x{value:04x})")
        
        # Send heartbeat after initial batch
        client.send_heartbeat()
        
        # Read remaining registers in batches of 20
        max_retries = 3
        retry_delay = 1.0  # 1 second between retries
        consecutive_failures = 0
        max_consecutive_failures = 3  # Stop after 3 consecutive failures
        
        # Start from register 20 and read in batches of 20
        for start_reg in range(20, 65536, 20):  # Read up to register 65535 (or until we hit max failures)
            end_reg = min(start_reg + 20, 65536)  # Ensure we don't exceed 65535
            count = min(20, 65536 - start_reg)  # Adjust count for last batch
            
            logger.info(f"Reading registers {start_reg} to {end_reg-1}")
            
            # Try reading with retries
            success = False
            for retry in range(max_retries):
                try:
                    # Send heartbeat before reading to keep connection alive
                    client.check_and_send_heartbeat()
                    
                    response = client.read_holding_registers(start_reg, count)
                    if not response:
                        raise Exception("No response received")
                        
                    # Process this batch of registers
                    for i in range(0, len(response), 2):
                        reg_num = start_reg + (i // 2)
                        
                        # Read 2 bytes as little-endian value
                        value = response[i] | (response[i+1] << 8)
                        reg_def = get_register_definition(registers, 'hold', reg_num)
                        
                        if reg_def:
                            decoded_value = decode_register_value(reg_def, value)
                            config_data[reg_def['shortname']] = {
                                'name': reg_def['name'],
                                'value': decoded_value,
                                'unit': reg_def.get('unit', ''),
                                'description': reg_def['description']
                            }
                            logger.info(f"Register {reg_num:5d}: {reg_def['shortname']:<30} = {value:5d} (0x{value:04x}) -> {decoded_value}")
                        else:
                            logger.warning(f"Register {reg_num:5d}: Unknown register, raw value = {value:5d} (0x{value:04x})")
                    
                    # Successfully read this batch
                    success = True
                    consecutive_failures = 0  # Reset failure counter
                    
                    # Send heartbeat after successful batch read
                    client.send_heartbeat()
                    break
                    
                except Exception as e:
                    if retry < max_retries - 1:
                        logger.warning(f"Retry {retry + 1}/{max_retries} for registers {start_reg}-{end_reg-1}: {e}")
                        time.sleep(retry_delay)
                        # Try to reconnect if needed
                        if not client.sock:
                            logger.info("Attempting to reconnect...")
                            if not client.connect():
                                raise Exception("Failed to reconnect")
                    else:
                        logger.error(f"Failed to read registers {start_reg}-{end_reg-1} after {max_retries} retries: {e}")
                        consecutive_failures += 1
            
            if not success:
                if consecutive_failures >= max_consecutive_failures:
                    logger.info(f"Stopping after {max_consecutive_failures} consecutive failures")
                    break
                else:
                    logger.warning(f"Batch failed, will retry after delay")
                    time.sleep(retry_delay)
                    continue
            
            # Add a small delay between batches
            time.sleep(0.2)
        
        if config_data:
            # Add received IDs to the configuration (convert bytearrays to hex strings)
            config_data['_received_datalog'] = client.datalog_id
            config_data['_received_serial'] = client.inverter_serial
            return config_data
            
    except Exception as e:
        logger.error(f"Error reading from port {port}: {e}")
        return None
    finally:
        if client:
            client.close()
            logger.info("Connection closed")

def backup_inverter_config(config_path: str, register_path: str, output_file: str = None):
    """Main function to backup inverter configurations."""
    # Load configuration and register definitions
    config = load_config(config_path)
    registers = load_registers(register_path)
    
    # Process each inverter
    for inverter in config.get('inverters', []):
        if not inverter.get('enabled', False):
            continue
            
        host = inverter.get('host')
        configured_port = inverter.get('port', 502)
        serial = inverter.get('serial')
        
        if not all([host, serial]):
            logger.warning(f"Skipping inverter with missing host or serial: {inverter}")
            continue
            
        logger.info(f"Processing inverter {serial} at {host}")
        
        # Try configured port first
        config_data = try_connect_and_read(host, configured_port, registers)
        
        if not config_data:
            logger.error(f"Failed to read configuration from inverter {serial}")
            continue
        
        # Format output
        output_data = {
            'inverter_serial': serial,
            'inverter_host': host,
            'timestamp': time.strftime('%Y-%m-%d %H:%M:%S'),
            'configuration': config_data
        }
        
        # Output to file or stdout
        if output_file:
            try:
                with open(output_file, 'w') as f:
                    json.dump(output_data, f, indent=2)
                logger.info(f"Saved configuration for inverter {serial} to {output_file}")
            except Exception as e:
                logger.error(f"Error writing to output file: {e}")
                # Fallback to stdout
                json.dump(output_data, sys.stdout, indent=2)
        else:
            json.dump(output_data, sys.stdout, indent=2)

def main():
    parser = argparse.ArgumentParser(description='Backup inverter configurations')
    parser.add_argument('--config', default='config.yaml', help='Path to config.yaml file')
    parser.add_argument('--registers', default='doc/eg4_registers.json', help='Path to register definitions file')
    parser.add_argument('--output', help='Output file path (if not specified, writes to stdout)')
    parser.add_argument('--debug', action='store_true', help='Enable debug logging')
    
    args = parser.parse_args()
    
    if args.debug:
        logger.setLevel(logging.DEBUG)
    
    # Test file existence
    if not Path(args.config).exists():
        logger.error(f"Config file not found: {args.config}")
        sys.exit(1)
    if not Path(args.registers).exists():
        logger.error(f"Register file not found: {args.registers}")
        sys.exit(1)
        
    backup_inverter_config(args.config, args.registers, args.output)

if __name__ == '__main__':
    main() 
