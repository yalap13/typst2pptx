"""
Python bindings for converting Typst documents to PowerPoint files.
"""

from .typst2pptx import typst_source_to_pptx, typst_to_pptx

__all__ = ["typst_source_to_pptx", "typst_to_pptx"]
