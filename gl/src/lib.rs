// pathfinder/gl/src/lib.rs
//
// Copyright © 2019 The Pathfinder Project Developers.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

//! An OpenGL implementation of the device abstraction.

#[macro_use]
extern crate log;

use gl::types::{GLboolean, GLchar, GLenum, GLfloat, GLint, GLsizei, GLsizeiptr, GLuint, GLvoid};
use pathfinder_geometry::rect::RectI;
use pathfinder_geometry::vector::Vector2I;
use pathfinder_gpu::resources::ResourceLoader;
use pathfinder_gpu::{RenderTarget, BlendState, BufferData, BufferTarget, BufferUploadMode};
use pathfinder_gpu::{ClearOps, DepthFunc, Device, Primitive, RenderOptions, RenderState};
use pathfinder_gpu::{ShaderKind, StencilFunc, TextureData, TextureFormat, UniformData};
use pathfinder_gpu::{VertexAttrClass, VertexAttrDescriptor, VertexAttrType};
use pathfinder_simd::default::F32x4;
use std::ffi::{CString, c_void};
use std::mem;
use std::ptr;
use std::str;
use std::time::Duration;

pub struct GLDevice {
    version: GLVersion,
    default_framebuffer: GLuint,
}

impl GLDevice {
    #[inline]
    pub fn load_with<F>(loadfn: F) where F: FnMut(&str) -> *const c_void {
        gl::load_with(loadfn)
    }

    #[inline]
    pub fn new(version: GLVersion, default_framebuffer: GLuint) -> GLDevice {
        GLDevice {
            version,
            default_framebuffer,
        }
    }

    pub fn set_default_framebuffer(&mut self, framebuffer: GLuint) {
        self.default_framebuffer = framebuffer;
    }

    fn set_texture_parameters(&self, texture: &GLTexture) {
        self.bind_texture(texture, 0);
        unsafe {
            gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MIN_FILTER, gl::LINEAR as GLint); ck();
            gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MAG_FILTER, gl::LINEAR as GLint); ck();
            gl::TexParameteri(gl::TEXTURE_2D,
                              gl::TEXTURE_WRAP_S,
                              gl::CLAMP_TO_EDGE as GLint); ck();
            gl::TexParameteri(gl::TEXTURE_2D,
                              gl::TEXTURE_WRAP_T,
                              gl::CLAMP_TO_EDGE as GLint); ck();
        }
    }

    fn set_render_state(&self, render_state: &RenderState<GLDevice>) {
        self.bind_render_target(render_state.target);

        unsafe {
            let (origin, size) = (render_state.viewport.origin(), render_state.viewport.size());
            gl::Viewport(origin.x(), origin.y(), size.x(), size.y());
        }

        if render_state.options.clear_ops.has_ops() {
            self.clear(&render_state.options.clear_ops);
        }

        self.use_program(render_state.program);
        self.bind_vertex_array(render_state.vertex_array);
        for (texture_unit, texture) in render_state.textures.iter().enumerate() {
            self.bind_texture(texture, texture_unit as u32);
        }

        render_state.uniforms.iter().for_each(|(uniform, data)| self.set_uniform(uniform, data));
        self.set_render_options(&render_state.options);
    }

    fn set_render_options(&self, render_options: &RenderOptions) {
        unsafe {
            // Set blend.
            match render_options.blend {
                BlendState::Off => {
                    gl::Disable(gl::BLEND); ck();
                }
                BlendState::RGBOneAlphaOne => {
                    gl::BlendEquation(gl::FUNC_ADD); ck();
                    gl::BlendFunc(gl::ONE, gl::ONE); ck();
                    gl::Enable(gl::BLEND); ck();
                }
                BlendState::RGBOneAlphaOneMinusSrcAlpha => {
                    gl::BlendEquation(gl::FUNC_ADD); ck();
                    gl::BlendFuncSeparate(gl::ONE,
                                          gl::ONE_MINUS_SRC_ALPHA,
                                          gl::ONE,
                                          gl::ONE); ck();
                    gl::Enable(gl::BLEND); ck();
                }
                BlendState::RGBSrcAlphaAlphaOneMinusSrcAlpha => {
                    gl::BlendEquation(gl::FUNC_ADD); ck();
                    gl::BlendFuncSeparate(gl::SRC_ALPHA,
                                          gl::ONE_MINUS_SRC_ALPHA,
                                          gl::ONE,
                                          gl::ONE); ck();
                    gl::Enable(gl::BLEND); ck();
                }
            }

            // Set depth.
            match render_options.depth {
                None => {
                    gl::Disable(gl::DEPTH_TEST); ck();
                }
                Some(ref state) => {
                    gl::DepthFunc(state.func.to_gl_depth_func()); ck();
                    gl::DepthMask(state.write as GLboolean); ck();
                    gl::Enable(gl::DEPTH_TEST); ck();
                }
            }

            // Set stencil.
            match render_options.stencil {
                None => {
                    gl::Disable(gl::STENCIL_TEST); ck();
                }
                Some(ref state) => {
                    gl::StencilFunc(state.func.to_gl_stencil_func(),
                                    state.reference as GLint,
                                    state.mask); ck();
                    let (pass_action, write_mask) = if state.write {
                        (gl::REPLACE, state.mask)
                    } else {
                        (gl::KEEP, 0)
                    };
                    gl::StencilOp(gl::KEEP, gl::KEEP, pass_action); ck();
                    gl::StencilMask(write_mask);
                    gl::Enable(gl::STENCIL_TEST); ck();
                }
            }

            // Set color mask.
            let color_mask = render_options.color_mask as GLboolean;
            gl::ColorMask(color_mask, color_mask, color_mask, color_mask); ck();
        }
    }

    fn set_uniform(&self, uniform: &GLUniform, data: &UniformData) {
        unsafe {
            match *data {
                UniformData::Float(value) => {
                    gl::Uniform1f(uniform.location, value); ck();
                }
                UniformData::Int(value) => {
                    gl::Uniform1i(uniform.location, value); ck();
                }
                UniformData::Mat2(data) => {
                    assert_eq!(mem::size_of::<F32x4>(), 4 * 4);
                    gl::UniformMatrix2fv(uniform.location,
                                         1,
                                         gl::FALSE,
                                         &data as *const F32x4 as *const GLfloat);
                }
                UniformData::Mat4(data) => {
                    assert_eq!(mem::size_of::<[F32x4; 4]>(), 4 * 4 * 4);
                    let data_ptr: *const F32x4 = data.as_ptr();
                    gl::UniformMatrix4fv(uniform.location,
                                         1,
                                         gl::FALSE,
                                         data_ptr as *const GLfloat);
                }
                UniformData::Vec2(data) => {
                    gl::Uniform2f(uniform.location, data.x(), data.y()); ck();
                }
                UniformData::Vec4(data) => {
                    gl::Uniform4f(uniform.location, data.x(), data.y(), data.z(), data.w()); ck();
                }
                UniformData::TextureUnit(unit) => {
                    gl::Uniform1i(uniform.location, unit as GLint); ck();
                }
            }
        }
    }

    fn reset_render_state(&self, render_state: &RenderState<GLDevice>) {
        self.reset_render_options(&render_state.options);
        for texture_unit in 0..(render_state.textures.len() as u32) {
            self.unbind_texture(texture_unit);
        }
        self.unuse_program();
        self.unbind_vertex_array();
    }

    fn reset_render_options(&self, render_options: &RenderOptions) {
        unsafe {
            match render_options.blend {
                BlendState::Off => {}
                BlendState::RGBOneAlphaOneMinusSrcAlpha |
                BlendState::RGBOneAlphaOne |
                BlendState::RGBSrcAlphaAlphaOneMinusSrcAlpha => {
                    gl::Disable(gl::BLEND); ck();
                }
            }

            if render_options.depth.is_some() {
                gl::Disable(gl::DEPTH_TEST); ck();
            }

            if render_options.stencil.is_some() {
                gl::StencilMask(!0); ck();
                gl::Disable(gl::STENCIL_TEST); ck();
            }

            gl::ColorMask(gl::TRUE, gl::TRUE, gl::TRUE, gl::TRUE); ck();
        }
    }
}

impl Device for GLDevice {
    type Buffer = GLBuffer;
    type Framebuffer = GLFramebuffer;
    type Program = GLProgram;
    type Shader = GLShader;
    type Texture = GLTexture;
    type TimerQuery = GLTimerQuery;
    type Uniform = GLUniform;
    type VertexArray = GLVertexArray;
    type VertexAttr = GLVertexAttr;

    fn create_texture(&self, format: TextureFormat, size: Vector2I) -> GLTexture {
        let mut texture = GLTexture { gl_texture: 0, size, format };
        unsafe {
            gl::GenTextures(1, &mut texture.gl_texture); ck();
            self.bind_texture(&texture, 0);
            gl::TexImage2D(gl::TEXTURE_2D,
                           0,
                           format.gl_internal_format(),
                           size.x() as GLsizei,
                           size.y() as GLsizei,
                           0,
                           format.gl_format(),
                           format.gl_type(),
                           ptr::null()); ck();
        }

        self.set_texture_parameters(&texture);
        texture
    }

    fn create_texture_from_data(&self, size: Vector2I, data: &[u8]) -> GLTexture {
        assert!(data.len() >= size.x() as usize * size.y() as usize);

        let mut texture = GLTexture { gl_texture: 0, size, format: TextureFormat::R8 };
        unsafe {
            gl::GenTextures(1, &mut texture.gl_texture); ck();
            self.bind_texture(&texture, 0);
            gl::TexImage2D(gl::TEXTURE_2D,
                           0,
                           gl::R8 as GLint,
                           size.x() as GLsizei,
                           size.y() as GLsizei,
                           0,
                           gl::RED,
                           gl::UNSIGNED_BYTE,
                           data.as_ptr() as *const GLvoid); ck();
        }

        self.set_texture_parameters(&texture);
        texture
    }

    fn create_shader_from_source(&self, name: &str, source: &[u8], kind: ShaderKind) -> GLShader {
        // FIXME(pcwalton): Do this once and cache it.
        let glsl_version_spec = self.version.to_glsl_version_spec();

        let mut output = vec![];
        self.preprocess(&mut output, source, glsl_version_spec);
        let source = output;

        let gl_shader_kind = match kind {
            ShaderKind::Vertex => gl::VERTEX_SHADER,
            ShaderKind::Fragment => gl::FRAGMENT_SHADER,
        };

        unsafe {
            let gl_shader = gl::CreateShader(gl_shader_kind); ck();
            gl::ShaderSource(gl_shader,
                             1,
                             [source.as_ptr() as *const GLchar].as_ptr(),
                             [source.len() as GLint].as_ptr()); ck();
            gl::CompileShader(gl_shader); ck();

            let mut compile_status = 0;
            gl::GetShaderiv(gl_shader, gl::COMPILE_STATUS, &mut compile_status); ck();
            if compile_status != gl::TRUE as GLint {
                let mut info_log_length = 0;
                gl::GetShaderiv(gl_shader, gl::INFO_LOG_LENGTH, &mut info_log_length); ck();
                let mut info_log = vec![0; info_log_length as usize];
                gl::GetShaderInfoLog(gl_shader,
                                     info_log.len() as GLint,
                                     ptr::null_mut(),
                                     info_log.as_mut_ptr() as *mut GLchar); ck();
                println!("Shader info log:\n{}", String::from_utf8_lossy(&info_log));
                panic!("{:?} shader '{}' compilation failed", kind, name);
            }

            GLShader { gl_shader }
        }
    }

    fn create_program_from_shaders(&self,
                                   _resources: &dyn ResourceLoader,
                                   name: &str,
                                   vertex_shader: GLShader,
                                   fragment_shader: GLShader)
                                   -> GLProgram {
        let gl_program;
        unsafe {
            gl_program = gl::CreateProgram(); ck();
            gl::AttachShader(gl_program, vertex_shader.gl_shader); ck();
            gl::AttachShader(gl_program, fragment_shader.gl_shader); ck();
            gl::LinkProgram(gl_program); ck();

            let mut link_status = 0;
            gl::GetProgramiv(gl_program, gl::LINK_STATUS, &mut link_status); ck();
            if link_status != gl::TRUE as GLint {
                let mut info_log_length = 0;
                gl::GetProgramiv(gl_program, gl::INFO_LOG_LENGTH, &mut info_log_length); ck();
                let mut info_log = vec![0; info_log_length as usize];
                gl::GetProgramInfoLog(gl_program,
                                      info_log.len() as GLint,
                                      ptr::null_mut(),
                                      info_log.as_mut_ptr() as *mut GLchar); ck();
                eprintln!("Program info log:\n{}", String::from_utf8_lossy(&info_log));
                panic!("Program '{}' linking failed", name);
            }
        }

        GLProgram { gl_program, vertex_shader, fragment_shader }
    }

    #[inline]
    fn create_vertex_array(&self) -> GLVertexArray {
        unsafe {
            let mut array = GLVertexArray { gl_vertex_array: 0 };
            gl::GenVertexArrays(1, &mut array.gl_vertex_array); ck();
            array
        }
    }

    fn get_vertex_attr(&self, program: &Self::Program, name: &str) -> Option<GLVertexAttr> {
        let name = CString::new(format!("a{}", name)).unwrap();
        let attr = unsafe {
            gl::GetAttribLocation(program.gl_program, name.as_ptr() as *const GLchar)
        }; ck();
        if attr < 0 {
            None
        } else {
            Some(GLVertexAttr { attr: attr as GLuint })
        }
    }

    fn get_uniform(&self, program: &GLProgram, name: &str) -> GLUniform {
        let name = CString::new(format!("u{}", name)).unwrap();
        let location = unsafe {
            gl::GetUniformLocation(program.gl_program, name.as_ptr() as *const GLchar)
        }; ck();
        GLUniform { location }
    }

    fn configure_vertex_attr(&self,
                             vertex_array: &GLVertexArray,
                             attr: &GLVertexAttr,
                             descriptor: &VertexAttrDescriptor) {
        debug_assert_ne!(descriptor.stride, 0);

        self.bind_vertex_array(vertex_array);

        unsafe {
            let attr_type = descriptor.attr_type.to_gl_type();
            match descriptor.class {
                VertexAttrClass::Float | VertexAttrClass::FloatNorm => {
                    let normalized = if descriptor.class == VertexAttrClass::FloatNorm {
                        gl::TRUE
                    } else {
                        gl::FALSE
                    };
                    gl::VertexAttribPointer(attr.attr,
                                            descriptor.size as GLint,
                                            attr_type,
                                            normalized,
                                            descriptor.stride as GLint,
                                            descriptor.offset as *const GLvoid); ck();
                }
                VertexAttrClass::Int => {
                    gl::VertexAttribIPointer(attr.attr,
                                             descriptor.size as GLint,
                                             attr_type,
                                             descriptor.stride as GLint,
                                             descriptor.offset as *const GLvoid); ck();
                }
            }

            gl::VertexAttribDivisor(attr.attr, descriptor.divisor); ck();
            gl::EnableVertexAttribArray(attr.attr); ck();
        }

        self.unbind_vertex_array();
    }

    fn create_framebuffer(&self, texture: GLTexture) -> GLFramebuffer {
        let mut gl_framebuffer = 0;
        unsafe {
            gl::GenFramebuffers(1, &mut gl_framebuffer); ck();
            gl::BindFramebuffer(gl::FRAMEBUFFER, gl_framebuffer); ck();
            self.bind_texture(&texture, 0);
            gl::FramebufferTexture2D(gl::FRAMEBUFFER,
                                     gl::COLOR_ATTACHMENT0,
                                     gl::TEXTURE_2D,
                                     texture.gl_texture,
                                     0); ck();
            assert_eq!(gl::CheckFramebufferStatus(gl::FRAMEBUFFER), gl::FRAMEBUFFER_COMPLETE);
        }

        GLFramebuffer { gl_framebuffer, texture }
    }

    fn create_buffer(&self) -> GLBuffer {
        unsafe {
            let mut gl_buffer = 0;
            gl::GenBuffers(1, &mut gl_buffer); ck();
            GLBuffer { gl_buffer }
        }
    }

    fn allocate_buffer<T>(&self,
                          buffer: &GLBuffer,
                          data: BufferData<T>,
                          target: BufferTarget,
                          mode: BufferUploadMode) {
        let target = match target {
            BufferTarget::Vertex => gl::ARRAY_BUFFER,
            BufferTarget::Index => gl::ELEMENT_ARRAY_BUFFER,
        };
        let (ptr, len) = match data {
            BufferData::Uninitialized(len) => (ptr::null(), len),
            BufferData::Memory(buffer) => (buffer.as_ptr() as *const GLvoid, buffer.len()),
        };
        let len = (len * mem::size_of::<T>()) as GLsizeiptr;
        let usage = mode.to_gl_usage();
        unsafe {
            gl::BindBuffer(target, buffer.gl_buffer); ck();
            gl::BufferData(target, len, ptr, usage); ck();
        }
    }

    #[inline]
    fn framebuffer_texture<'f>(&self, framebuffer: &'f Self::Framebuffer) -> &'f Self::Texture {
        &framebuffer.texture
    }

    #[inline]
    fn texture_size(&self, texture: &Self::Texture) -> Vector2I {
        texture.size
    }

    fn upload_to_texture(&self, texture: &Self::Texture, size: Vector2I, data: &[u8]) {
        assert!(data.len() >= size.x() as usize * size.y() as usize * 4);
        unsafe {
            self.bind_texture(texture, 0);
            gl::TexImage2D(gl::TEXTURE_2D,
                           0,
                           gl::RGBA as GLint,
                           size.x() as GLsizei,
                           size.y() as GLsizei,
                           0,
                           gl::RGBA,
                           gl::UNSIGNED_BYTE,
                           data.as_ptr() as *const GLvoid); ck();
        }

        self.set_texture_parameters(texture);
    }

    fn read_pixels(&self, render_target: &RenderTarget<GLDevice>, viewport: RectI) -> TextureData {
        let (origin, size) = (viewport.origin(), viewport.size());
        let format = self.render_target_format(render_target);
        self.bind_render_target(render_target);

        match format {
            TextureFormat::R8 | TextureFormat::RGBA8 => {
                let channels = format.channels();
                let mut pixels = vec![0; size.x() as usize * size.y() as usize * channels];
                unsafe {
                    gl::ReadPixels(origin.x(),
                                   origin.y(),
                                   size.x() as GLsizei,
                                   size.y() as GLsizei,
                                   format.gl_format(),
                                   format.gl_type(),
                                   pixels.as_mut_ptr() as *mut GLvoid); ck();
                }
                flip_y(&mut pixels, size, channels);
                TextureData::U8(pixels)
            }
            TextureFormat::R16F => {
                let mut pixels = vec![0; size.x() as usize * size.y() as usize];
                unsafe {
                    gl::ReadPixels(origin.x(),
                                   origin.y(),
                                   size.x() as GLsizei,
                                   size.y() as GLsizei,
                                   format.gl_format(),
                                   format.gl_type(),
                                   pixels.as_mut_ptr() as *mut GLvoid); ck();
                }
                flip_y(&mut pixels, size, 1);
                TextureData::U16(pixels)
            }
        }
    }

    fn begin_commands(&self) {
        // TODO(pcwalton): Add some checks in debug mode to make sure render commands are bracketed
        // by these?
    }

    fn end_commands(&self) {
        unsafe { gl::Flush(); }
    }

    fn draw_arrays(&self, index_count: u32, render_state: &RenderState<Self>) {
        self.set_render_state(render_state);
        unsafe {
            gl::DrawArrays(render_state.primitive.to_gl_primitive(),
                           0,
                           index_count as GLsizei); ck();
        }
        self.reset_render_state(render_state);
    }

    fn draw_elements(&self, index_count: u32, render_state: &RenderState<Self>) {
        self.set_render_state(render_state);
        unsafe {
            gl::DrawElements(render_state.primitive.to_gl_primitive(),
                             index_count as GLsizei,
                             gl::UNSIGNED_INT,
                             ptr::null()); ck();
        }
        self.reset_render_state(render_state);
    }

    fn draw_elements_instanced(&self,
                               index_count: u32,
                               instance_count: u32,
                               render_state: &RenderState<Self>) {
        self.set_render_state(render_state);
        unsafe {
            gl::DrawElementsInstanced(render_state.primitive.to_gl_primitive(),
                                      index_count as GLsizei,
                                      gl::UNSIGNED_INT,
                                      ptr::null(),
                                      instance_count as GLsizei); ck();
        }
        self.reset_render_state(render_state);
    }

    #[inline]
    fn create_timer_query(&self) -> GLTimerQuery {
        let mut query = GLTimerQuery { gl_query: 0 };
        unsafe {
            gl::GenQueries(1, &mut query.gl_query); ck();
        }
        query
    }

    #[inline]
    fn begin_timer_query(&self, query: &Self::TimerQuery) {
        unsafe {
            gl::BeginQuery(gl::TIME_ELAPSED, query.gl_query); ck();
        }
    }

    #[inline]
    fn end_timer_query(&self, _: &Self::TimerQuery) {
        unsafe {
            gl::EndQuery(gl::TIME_ELAPSED); ck();
        }
    }

    #[inline]
    fn get_timer_query(&self, query: &Self::TimerQuery) -> Option<Duration> {
        unsafe {
            let mut result = 0;
            gl::GetQueryObjectiv(query.gl_query, gl::QUERY_RESULT_AVAILABLE, &mut result); ck();
            if result == gl::FALSE as GLint {
                return None;
            }
            let mut result = 0;
            gl::GetQueryObjectui64v(query.gl_query, gl::QUERY_RESULT, &mut result); ck();
            Some(Duration::from_nanos(result))
        }
    }

    #[inline]
    fn bind_buffer(&self, vertex_array: &GLVertexArray, buffer: &GLBuffer, target: BufferTarget) {
        self.bind_vertex_array(vertex_array);
        unsafe {
            gl::BindBuffer(target.to_gl_target(), buffer.gl_buffer); ck();
        }
        self.unbind_vertex_array();
    }

    #[inline]
    fn create_shader(
        &self,
        resources: &dyn ResourceLoader,
        name: &str,
        kind: ShaderKind,
    ) -> Self::Shader {
        let suffix = match kind {
            ShaderKind::Vertex => 'v',
            ShaderKind::Fragment => 'f',
        };
        let path = format!("shaders/gl3/{}.{}s.glsl", name, suffix);
        self.create_shader_from_source(name, &resources.slurp(&path).unwrap(), kind)
    }
}

impl GLDevice {
    fn bind_render_target(&self, attachment: &RenderTarget<GLDevice>) {
        match *attachment {
            RenderTarget::Default => self.bind_default_framebuffer(),
            RenderTarget::Framebuffer(framebuffer) => self.bind_framebuffer(framebuffer),
        }
    }

    fn bind_vertex_array(&self, vertex_array: &GLVertexArray) {
        unsafe {
            gl::BindVertexArray(vertex_array.gl_vertex_array); ck();
        }
    }

    fn unbind_vertex_array(&self) {
        unsafe {
            gl::BindVertexArray(0); ck();
        }
    }

    pub fn create_texture_from_existing_texture(&self, texture_id: GLuint,format: TextureFormat, size: Vector2I) -> GLTexture {
        let mut texture = GLTexture { gl_texture: texture_id, size, format };
        unsafe {
            self.bind_texture(&texture, 0);
        }
        self.set_texture_parameters(&texture);
        texture
    }

    fn bind_texture(&self, texture: &GLTexture, unit: u32) {
        unsafe {
            gl::ActiveTexture(gl::TEXTURE0 + unit); ck();
            gl::BindTexture(gl::TEXTURE_2D, texture.gl_texture); ck();
        }
    }

    fn unbind_texture(&self, unit: u32) {
        unsafe {
            gl::ActiveTexture(gl::TEXTURE0 + unit); ck();
            gl::BindTexture(gl::TEXTURE_2D, 0); ck();
        }
    }

    fn use_program(&self, program: &GLProgram) {
        unsafe {
            gl::UseProgram(program.gl_program); ck();
        }
    }

    fn unuse_program(&self) {
        unsafe {
            gl::UseProgram(0); ck();
        }
    }

    fn bind_default_framebuffer(&self) {
        unsafe {
            gl::BindFramebuffer(gl::FRAMEBUFFER, self.default_framebuffer); ck();
        }
    }

    fn bind_framebuffer(&self, framebuffer: &GLFramebuffer) {
        unsafe {
            gl::BindFramebuffer(gl::FRAMEBUFFER, framebuffer.gl_framebuffer); ck();
        }
    }

    fn preprocess(&self, output: &mut Vec<u8>, source: &[u8], version: &str) {
        let mut index = 0;
        while index < source.len() {
            if source[index..].starts_with(b"{{") {
                let end_index = source[index..].iter()
                                               .position(|character| *character == b'}')
                                               .expect("Expected `}`!") + index;
                assert_eq!(source[end_index + 1], b'}');
                let ident = String::from_utf8_lossy(&source[(index + 2)..end_index]);
                if ident == "version" {
                    output.extend_from_slice(version.as_bytes());
                } else {
                    panic!("unknown template variable: `{}`", ident);
                }
                index = end_index + 2;
            } else {
                output.push(source[index]);
                index += 1;
            }
        }
    }

    fn clear(&self, ops: &ClearOps) {
        unsafe {
            let mut flags = 0;
            if let Some(color) = ops.color {
                gl::ColorMask(gl::TRUE, gl::TRUE, gl::TRUE, gl::TRUE); ck();
                gl::ClearColor(color.r(), color.g(), color.b(), color.a()); ck();
                flags |= gl::COLOR_BUFFER_BIT;
            }
            if let Some(depth) = ops.depth {
                gl::DepthMask(gl::TRUE); ck();
                gl::ClearDepthf(depth as _); ck(); // FIXME(pcwalton): GLES
                flags |= gl::DEPTH_BUFFER_BIT;
            }
            if let Some(stencil) = ops.stencil {
                gl::StencilMask(!0); ck();
                gl::ClearStencil(stencil as GLint); ck();
                flags |= gl::STENCIL_BUFFER_BIT;
            }
            if flags != 0 {
                gl::Clear(flags); ck();
            }
        }
    }

    fn render_target_format(&self, render_target: &RenderTarget<GLDevice>) -> TextureFormat {
        match *render_target {
            RenderTarget::Default => TextureFormat::RGBA8,
            RenderTarget::Framebuffer(ref framebuffer) => {
                self.framebuffer_texture(framebuffer).format
            }
        }
    }
}

pub struct GLVertexArray {
    pub gl_vertex_array: GLuint,
}

impl Drop for GLVertexArray {
    #[inline]
    fn drop(&mut self) {
        unsafe {
            gl::DeleteVertexArrays(1, &mut self.gl_vertex_array); ck();
        }
    }
}

pub struct GLVertexAttr {
    attr: GLuint,
}

impl GLVertexAttr {
    pub fn configure_float(&self,
                           size: GLint,
                           gl_type: GLuint,
                           normalized: bool,
                           stride: GLsizei,
                           offset: usize,
                           divisor: GLuint) {
        unsafe {
            gl::VertexAttribPointer(self.attr,
                                    size,
                                    gl_type,
                                    if normalized { gl::TRUE } else { gl::FALSE },
                                    stride,
                                    offset as *const GLvoid); ck();
            gl::VertexAttribDivisor(self.attr, divisor); ck();
            gl::EnableVertexAttribArray(self.attr); ck();
        }
    }

    pub fn configure_int(&self,
                         size: GLint,
                         gl_type: GLuint,
                         stride: GLsizei,
                         offset: usize,
                         divisor: GLuint) {
        unsafe {
            gl::VertexAttribIPointer(self.attr,
                                     size,
                                     gl_type,
                                     stride,
                                     offset as *const GLvoid); ck();
            gl::VertexAttribDivisor(self.attr, divisor); ck();
            gl::EnableVertexAttribArray(self.attr); ck();
        }
    }
}

pub struct GLFramebuffer {
    pub gl_framebuffer: GLuint,
    pub texture: GLTexture,
}

impl Drop for GLFramebuffer {
    fn drop(&mut self) {
        unsafe {
            gl::DeleteFramebuffers(1, &mut self.gl_framebuffer); ck();
        }
    }
}

pub struct GLBuffer {
    pub gl_buffer: GLuint,
}

impl Drop for GLBuffer {
    fn drop(&mut self) {
        unsafe {
            gl::DeleteBuffers(1, &mut self.gl_buffer); ck();
        }
    }
}

#[derive(Debug)]
pub struct GLUniform {
    location: GLint,
}

pub struct GLProgram {
    pub gl_program: GLuint,
    #[allow(dead_code)]
    vertex_shader: GLShader,
    #[allow(dead_code)]
    fragment_shader: GLShader,
}

impl Drop for GLProgram {
    fn drop(&mut self) {
        unsafe {
            gl::DeleteProgram(self.gl_program); ck();
        }
    }
}

pub struct GLShader {
    gl_shader: GLuint,
}

impl Drop for GLShader {
    fn drop(&mut self) {
        unsafe {
            gl::DeleteShader(self.gl_shader); ck();
        }
    }
}

pub struct GLTexture {
    pub gl_texture: GLuint,
    pub size: Vector2I,
    pub format: TextureFormat,
}

pub struct GLTimerQuery {
    gl_query: GLuint,
}

impl Drop for GLTimerQuery {
    #[inline]
    fn drop(&mut self) {
        unsafe {
            gl::DeleteQueries(1, &mut self.gl_query); ck();
        }
    }
}

trait BufferTargetExt {
    fn to_gl_target(self) -> GLuint;
}

impl BufferTargetExt for BufferTarget {
    fn to_gl_target(self) -> GLuint {
        match self {
            BufferTarget::Vertex => gl::ARRAY_BUFFER,
            BufferTarget::Index => gl::ELEMENT_ARRAY_BUFFER,
        }
    }
}

trait BufferUploadModeExt {
    fn to_gl_usage(self) -> GLuint;
}

impl BufferUploadModeExt for BufferUploadMode {
    fn to_gl_usage(self) -> GLuint {
        match self {
            BufferUploadMode::Static => gl::STATIC_DRAW,
            BufferUploadMode::Dynamic => gl::DYNAMIC_DRAW,
        }
    }
}

trait DepthFuncExt {
    fn to_gl_depth_func(self) -> GLenum;
}

impl DepthFuncExt for DepthFunc {
    fn to_gl_depth_func(self) -> GLenum {
        match self {
            DepthFunc::Less => gl::LESS,
            DepthFunc::Always => gl::ALWAYS,
        }
    }
}

trait PrimitiveExt {
    fn to_gl_primitive(self) -> GLuint;
}

impl PrimitiveExt for Primitive {
    fn to_gl_primitive(self) -> GLuint {
        match self {
            Primitive::Triangles => gl::TRIANGLES,
            Primitive::Lines => gl::LINES,
        }
    }
}

trait StencilFuncExt {
    fn to_gl_stencil_func(self) -> GLenum;
}

impl StencilFuncExt for StencilFunc {
    fn to_gl_stencil_func(self) -> GLenum {
        match self {
            StencilFunc::Always => gl::ALWAYS,
            StencilFunc::Equal => gl::EQUAL,
        }
    }
}

trait TextureFormatExt {
    fn gl_internal_format(self) -> GLint;
    fn gl_format(self) -> GLuint;
    fn gl_type(self) -> GLuint;
}

impl TextureFormatExt for TextureFormat {
    fn gl_internal_format(self) -> GLint {
        match self {
            TextureFormat::R8 => gl::R8 as GLint,
            TextureFormat::R16F => gl::R16F as GLint,
            TextureFormat::RGBA8 => gl::RGBA as GLint,
        }
    }

    fn gl_format(self) -> GLuint {
        match self {
            TextureFormat::R8 | TextureFormat::R16F => gl::RED,
            TextureFormat::RGBA8 => gl::RGBA,
        }
    }

    fn gl_type(self) -> GLuint {
        match self {
            TextureFormat::R8 | TextureFormat::RGBA8 => gl::UNSIGNED_BYTE,
            TextureFormat::R16F => gl::HALF_FLOAT,
        }
    }
}

trait VertexAttrTypeExt {
    fn to_gl_type(self) -> GLuint;
}

impl VertexAttrTypeExt for VertexAttrType {
    fn to_gl_type(self) -> GLuint {
        match self {
            VertexAttrType::F32 => gl::FLOAT,
            VertexAttrType::I16 => gl::SHORT,
            VertexAttrType::I8  => gl::BYTE,
            VertexAttrType::U16 => gl::UNSIGNED_SHORT,
            VertexAttrType::U8  => gl::UNSIGNED_BYTE,
        }
    }
}

/// The version/dialect of OpenGL we should render with.
#[derive(Clone, Copy)]
#[repr(u32)]
pub enum GLVersion {
    /// OpenGL 3.0+, core profile.
    GL3 = 0,
    /// OpenGL ES 3.0+.
    GLES3 = 1,
}

impl GLVersion {
    fn to_glsl_version_spec(&self) -> &'static str {
        match *self {
            GLVersion::GL3 => "330",
            GLVersion::GLES3 => "300 es",
        }
    }
}

// Error checking

#[cfg(debug_assertions)]
fn ck() {
    unsafe {
        // Note that ideally we should be calling gl::GetError() in a loop until it
        // returns gl::NO_ERROR, but for now we'll just report the first one we find.
        let err = gl::GetError();
        if err != gl::NO_ERROR {
            panic!("GL error: 0x{:x} ({})", err, match err {
                gl::INVALID_ENUM => "INVALID_ENUM",
                gl::INVALID_VALUE => "INVALID_VALUE",
                gl::INVALID_OPERATION => "INVALID_OPERATION",
                gl::INVALID_FRAMEBUFFER_OPERATION => "INVALID_FRAMEBUFFER_OPERATION",
                gl::OUT_OF_MEMORY => "OUT_OF_MEMORY",
                gl::STACK_UNDERFLOW => "STACK_UNDERFLOW",
                gl::STACK_OVERFLOW => "STACK_OVERFLOW",
                _ => "Unknown"
            });
        }
    }
}

#[cfg(not(debug_assertions))]
fn ck() {}

// Utilities

// Flips a buffer of image data upside-down.
fn flip_y<T>(pixels: &mut [T], size: Vector2I, channels: usize) {
    let stride = size.x() as usize * channels;
    for y in 0..(size.y() as usize / 2) {
        let (index_a, index_b) = (y * stride, (size.y() as usize - y - 1) * stride);
        for offset in 0..stride {
            pixels.swap(index_a + offset, index_b + offset);
        }
    }
}
