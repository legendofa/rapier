use super::{ContactConstraintTypes, ContactPointInfos};
use crate::dynamics::solver::SolverVel;
use crate::dynamics::solver::{AnyConstraintMut, SolverBody};

use crate::dynamics::{IntegrationParameters, MultibodyJointSet, RigidBodySet};
use crate::geometry::{ContactManifold, ContactManifoldIndex};
use crate::math::*;
use crate::utils::{self, SimdAngularInertia, SimdBasis, SimdCross, SimdDot, SimdRealCopy};
use na::DVector;

use super::{TwoBodyConstraintElement, TwoBodyConstraintNormalPart};

impl<'a> AnyConstraintMut<'a, ContactConstraintTypes> {
    pub fn remove_bias(&mut self) {
        match self {
            Self::OneBody(c) => c.remove_cfm_and_bias_from_rhs(),
            Self::TwoBodies(c) => c.remove_cfm_and_bias_from_rhs(),
            Self::GenericOneBody(c) => c.remove_cfm_and_bias_from_rhs(),
            Self::GenericTwoBodies(c) => c.remove_cfm_and_bias_from_rhs(),
            #[cfg(feature = "simd-is-enabled")]
            Self::SimdOneBody(c) => c.remove_cfm_and_bias_from_rhs(),
            #[cfg(feature = "simd-is-enabled")]
            Self::SimdTwoBodies(c) => c.remove_cfm_and_bias_from_rhs(),
        }
    }

    pub fn solve_restitution(
        &mut self,
        generic_jacobians: &DVector<Real>,
        solver_vels: &mut [SolverVel<Real>],
        generic_solver_vels: &mut DVector<Real>,
    ) {
        match self {
            Self::OneBody(c) => c.solve(solver_vels, true, false),
            Self::TwoBodies(c) => c.solve(solver_vels, true, false),
            Self::GenericOneBody(c) => c.solve(generic_jacobians, generic_solver_vels, true, false),
            Self::GenericTwoBodies(c) => c.solve(
                generic_jacobians,
                solver_vels,
                generic_solver_vels,
                true,
                false,
            ),
            #[cfg(feature = "simd-is-enabled")]
            Self::SimdOneBody(c) => c.solve(solver_vels, true, false),
            #[cfg(feature = "simd-is-enabled")]
            Self::SimdTwoBodies(c) => c.solve(solver_vels, true, false),
        }
    }

    pub fn solve_friction(
        &mut self,
        generic_jacobians: &DVector<Real>,
        solver_vels: &mut [SolverVel<Real>],
        generic_solver_vels: &mut DVector<Real>,
    ) {
        match self {
            Self::OneBody(c) => c.solve(solver_vels, false, true),
            Self::TwoBodies(c) => c.solve(solver_vels, false, true),
            Self::GenericOneBody(c) => c.solve(generic_jacobians, generic_solver_vels, false, true),
            Self::GenericTwoBodies(c) => c.solve(
                generic_jacobians,
                solver_vels,
                generic_solver_vels,
                false,
                true,
            ),
            #[cfg(feature = "simd-is-enabled")]
            Self::SimdOneBody(c) => c.solve(solver_vels, false, true),
            #[cfg(feature = "simd-is-enabled")]
            Self::SimdTwoBodies(c) => c.solve(solver_vels, false, true),
        }
    }

    pub fn writeback_impulses(&mut self, manifolds_all: &mut [&mut ContactManifold]) {
        match self {
            Self::OneBody(c) => c.writeback_impulses(manifolds_all),
            Self::TwoBodies(c) => c.writeback_impulses(manifolds_all),
            Self::GenericOneBody(c) => c.writeback_impulses(manifolds_all),
            Self::GenericTwoBodies(c) => c.writeback_impulses(manifolds_all),
            #[cfg(feature = "simd-is-enabled")]
            Self::SimdOneBody(c) => c.writeback_impulses(manifolds_all),
            #[cfg(feature = "simd-is-enabled")]
            Self::SimdTwoBodies(c) => c.writeback_impulses(manifolds_all),
        }
    }
}

#[derive(Copy, Clone, Debug)]
pub(crate) struct TwoBodyConstraint {
    pub dir1: Vector, // Non-penetration force direction for the first body.
    #[cfg(feature = "dim3")]
    pub tangent1: Vector, // One of the friction force directions.
    pub im1: Vector,
    pub im2: Vector,
    pub cfm_factor: Real,
    pub limit: Real,
    pub solver_vel1: usize,
    pub solver_vel2: usize,
    pub manifold_id: ContactManifoldIndex,
    pub manifold_contact_id: [u8; MAX_MANIFOLD_POINTS],
    pub num_contacts: u8,
    pub elements: [TwoBodyConstraintElement<Real>; MAX_MANIFOLD_POINTS],
}

impl TwoBodyConstraint {
    pub fn invalid() -> Self {
        Self {
            dir1: Vector::zeros(),
            #[cfg(feature = "dim3")]
            tangent1: Vector::zeros(),
            im1: Vector::zeros(),
            im2: Vector::zeros(),
            cfm_factor: 0.0,
            limit: 0.0,
            solver_vel1: usize::MAX,
            solver_vel2: usize::MAX,
            manifold_id: ContactManifoldIndex::MAX,
            manifold_contact_id: [u8::MAX; MAX_MANIFOLD_POINTS],
            num_contacts: u8::MAX,
            elements: [TwoBodyConstraintElement::zero(); MAX_MANIFOLD_POINTS],
        }
    }
}

#[derive(Copy, Clone, Debug)]
pub(crate) struct TwoBodyConstraintBuilder {
    pub infos: [ContactPointInfos<Real>; MAX_MANIFOLD_POINTS],
}

impl TwoBodyConstraintBuilder {
    pub fn invalid() -> Self {
        Self {
            infos: [ContactPointInfos::default(); MAX_MANIFOLD_POINTS],
        }
    }

    pub fn generate(
        manifold_id: ContactManifoldIndex,
        manifold: &ContactManifold,
        bodies: &RigidBodySet,
        out_builders: &mut [TwoBodyConstraintBuilder],
        out_constraints: &mut [TwoBodyConstraint],
    ) {
        assert_eq!(manifold.data.relative_dominance, 0);

        let handle1 = manifold.data.rigid_body1.unwrap();
        let handle2 = manifold.data.rigid_body2.unwrap();

        let rb1 = &bodies[handle1];
        let (vels1, mprops1) = (&rb1.vels, &rb1.mprops);
        let rb2 = &bodies[handle2];
        let (vels2, mprops2) = (&rb2.vels, &rb2.mprops);

        let solver_vel1 = rb1.ids.active_set_offset;
        let solver_vel2 = rb2.ids.active_set_offset;
        let force_dir1 = -manifold.data.normal;

        #[cfg(feature = "dim2")]
        let tangents1 = force_dir1.orthonormal_basis();
        #[cfg(feature = "dim3")]
        let tangents1 =
            super::compute_tangent_contact_directions(&force_dir1, &vels1.linvel, &vels2.linvel);

        for (l, manifold_points) in manifold
            .data
            .solver_contacts
            .chunks(MAX_MANIFOLD_POINTS)
            .enumerate()
        {
            let builder = &mut out_builders[l];
            let constraint = &mut out_constraints[l];
            constraint.dir1 = force_dir1;
            constraint.im1 = mprops1.effective_inv_mass;
            constraint.im2 = mprops2.effective_inv_mass;
            constraint.solver_vel1 = solver_vel1;
            constraint.solver_vel2 = solver_vel2;
            constraint.manifold_id = manifold_id;
            constraint.num_contacts = manifold_points.len() as u8;
            #[cfg(feature = "dim3")]
            {
                constraint.tangent1 = tangents1[0];
            }

            for k in 0..manifold_points.len() {
                let manifold_point = &manifold_points[k];
                let point = manifold_point.point;

                let dp1 = point - mprops1.world_com;
                let dp2 = point - mprops2.world_com;

                let vel1 = vels1.linvel + vels1.angvel.gcross(dp1);
                let vel2 = vels2.linvel + vels2.angvel.gcross(dp2);

                constraint.limit = manifold_point.friction;
                constraint.manifold_contact_id[k] = manifold_point.contact_id;

                // Normal part.
                let normal_rhs_wo_bias;
                {
                    let gcross1 = mprops1
                        .effective_world_inv_inertia_sqrt
                        .transform_vector(dp1.gcross(force_dir1));
                    let gcross2 = mprops2
                        .effective_world_inv_inertia_sqrt
                        .transform_vector(dp2.gcross(-force_dir1));

                    let imsum = mprops1.effective_inv_mass + mprops2.effective_inv_mass;
                    let projected_mass = utils::inv(
                        force_dir1.dot(imsum.component_mul(&force_dir1))
                            + gcross1.gdot(gcross1)
                            + gcross2.gdot(gcross2),
                    );

                    let is_bouncy = manifold_point.is_bouncy() as u32 as Real;

                    normal_rhs_wo_bias =
                        (is_bouncy * manifold_point.restitution) * (vel1 - vel2).dot(force_dir1);

                    constraint.elements[k].normal_part = TwoBodyConstraintNormalPart {
                        gcross1,
                        gcross2,
                        rhs: na::zero(),
                        rhs_wo_bias: na::zero(),
                        impulse: na::zero(),
                        total_impulse: na::zero(),
                        r: projected_mass,
                    };
                }

                // Tangent parts.
                {
                    constraint.elements[k].tangent_part.impulse = Default::default();

                    for j in 0..DIM - 1 {
                        let gcross1 = mprops1
                            .effective_world_inv_inertia_sqrt
                            .transform_vector(dp1.gcross(tangents1[j]));
                        let gcross2 = mprops2
                            .effective_world_inv_inertia_sqrt
                            .transform_vector(dp2.gcross(-tangents1[j]));
                        let imsum = mprops1.effective_inv_mass + mprops2.effective_inv_mass;
                        let r = tangents1[j].dot(imsum.component_mul(&tangents1[j]))
                            + gcross1.gdot(gcross1)
                            + gcross2.gdot(gcross2);
                        let rhs_wo_bias = manifold_point.tangent_velocity.dot(tangents1[j]);

                        constraint.elements[k].tangent_part.gcross1[j] = gcross1;
                        constraint.elements[k].tangent_part.gcross2[j] = gcross2;
                        constraint.elements[k].tangent_part.rhs_wo_bias[j] = rhs_wo_bias;
                        constraint.elements[k].tangent_part.rhs[j] = rhs_wo_bias;
                        constraint.elements[k].tangent_part.r[j] = if cfg!(feature = "dim2") {
                            utils::inv(r)
                        } else {
                            r
                        };
                    }

                    #[cfg(feature = "dim3")]
                    {
                        constraint.elements[k].tangent_part.r[2] = 2.0
                            * (constraint.elements[k].tangent_part.gcross1[0]
                                .gdot(constraint.elements[k].tangent_part.gcross1[1])
                                + constraint.elements[k].tangent_part.gcross2[0]
                                    .gdot(constraint.elements[k].tangent_part.gcross2[1]));
                    }
                }

                // Builder.
                let infos = ContactPointInfos {
                    local_p1: rb1
                        .pos
                        .position
                        .inverse_transform_point(&manifold_point.point),
                    local_p2: rb2
                        .pos
                        .position
                        .inverse_transform_point(&manifold_point.point),
                    tangent_vel: manifold_point.tangent_velocity,
                    dist: manifold_point.dist,
                    normal_rhs_wo_bias,
                };

                builder.infos[k] = infos;
                constraint.manifold_contact_id[k] = manifold_point.contact_id;
            }
        }
    }

    pub fn update(
        &self,
        params: &IntegrationParameters,
        solved_dt: Real,
        bodies: &[SolverBody],
        _multibodies: &MultibodyJointSet,
        constraint: &mut TwoBodyConstraint,
    ) {
        let rb1 = &bodies[constraint.solver_vel1];
        let rb2 = &bodies[constraint.solver_vel2];
        let ccd_thickness = rb1.ccd_thickness + rb2.ccd_thickness;
        self.update_with_positions(
            params,
            solved_dt,
            &rb1.position,
            &rb2.position,
            ccd_thickness,
            constraint,
        )
    }

    // Used by both generic and non-generic builders..
    pub fn update_with_positions(
        &self,
        params: &IntegrationParameters,
        solved_dt: Real,
        rb1_pos: &Isometry,
        rb2_pos: &Isometry,
        ccd_thickness: Real,
        constraint: &mut TwoBodyConstraint,
    ) {
        let cfm_factor = params.cfm_factor();
        let inv_dt = params.inv_dt();
        let erp_inv_dt = params.erp_inv_dt();

        let all_infos = &self.infos[..constraint.num_contacts as usize];
        let all_elements = &mut constraint.elements[..constraint.num_contacts as usize];

        let mut is_fast_contact = false;

        #[cfg(feature = "dim2")]
        let tangents1 = constraint.dir1.orthonormal_basis();
        #[cfg(feature = "dim3")]
        let tangents1 = [
            constraint.tangent1,
            constraint.dir1.cross(constraint.tangent1),
        ];

        for (info, element) in all_infos.iter().zip(all_elements.iter_mut()) {
            // Tangent velocity is equivalent to the first body’s surface moving artificially.
            let p1 = rb1_pos.transform_point(&info.local_p1) + info.tangent_vel * solved_dt;
            let p2 = rb2_pos.transform_point(&info.local_p2);
            let dist = info.dist + (p1 - p2).dot(constraint.dir1);

            // Normal part.
            {
                let rhs_wo_bias = info.normal_rhs_wo_bias + dist.max(0.0) * inv_dt;
                let rhs_bias = erp_inv_dt
                    * (dist + params.allowed_linear_error)
                        .clamp(-params.max_penetration_correction, 0.0);
                let new_rhs = rhs_wo_bias + rhs_bias;
                let total_impulse = element.normal_part.total_impulse + element.normal_part.impulse;
                is_fast_contact = is_fast_contact || (-new_rhs * params.dt > ccd_thickness * 0.5);

                element.normal_part.rhs_wo_bias = rhs_wo_bias;
                element.normal_part.rhs = new_rhs;
                element.normal_part.total_impulse = total_impulse;
                element.normal_part.impulse = na::zero();
            }

            // Tangent part.
            {
                element.tangent_part.total_impulse += element.tangent_part.impulse;
                element.tangent_part.impulse = Default::default();

                for j in 0..DIM - 1 {
                    let bias = (p1 - p2).dot(tangents1[j]) * inv_dt;
                    element.tangent_part.rhs[j] = element.tangent_part.rhs_wo_bias[j] + bias;
                }
            }
        }

        constraint.cfm_factor = if is_fast_contact { 1.0 } else { cfm_factor };
    }
}

impl TwoBodyConstraint {
    pub fn solve(
        &mut self,
        solver_vels: &mut [SolverVel<Real>],
        solve_normal: bool,
        solve_friction: bool,
    ) {
        let mut solver_vel1 = solver_vels[self.solver_vel1];
        let mut solver_vel2 = solver_vels[self.solver_vel2];

        TwoBodyConstraintElement::solve_group(
            self.cfm_factor,
            &mut self.elements[..self.num_contacts as usize],
            &self.dir1,
            #[cfg(feature = "dim3")]
            &self.tangent1,
            &self.im1,
            &self.im2,
            self.limit,
            &mut solver_vel1,
            &mut solver_vel2,
            solve_normal,
            solve_friction,
        );

        solver_vels[self.solver_vel1] = solver_vel1;
        solver_vels[self.solver_vel2] = solver_vel2;
    }

    pub fn writeback_impulses(&self, manifolds_all: &mut [&mut ContactManifold]) {
        let manifold = &mut manifolds_all[self.manifold_id];

        for k in 0..self.num_contacts as usize {
            let contact_id = self.manifold_contact_id[k];
            let active_contact = &mut manifold.points[contact_id as usize];
            active_contact.data.impulse = self.elements[k].normal_part.impulse;

            #[cfg(feature = "dim2")]
            {
                active_contact.data.tangent_impulse = self.elements[k].tangent_part.impulse;
            }
            #[cfg(feature = "dim3")]
            {
                active_contact.data.tangent_impulse = self.elements[k].tangent_part.impulse;
            }
        }
    }

    pub fn remove_cfm_and_bias_from_rhs(&mut self) {
        self.cfm_factor = 1.0;
        for elt in &mut self.elements {
            elt.normal_part.rhs = elt.normal_part.rhs_wo_bias;
            // elt.normal_part.impulse = elt.normal_part.total_impulse;

            elt.tangent_part.rhs = elt.tangent_part.rhs_wo_bias;
            // elt.tangent_part.impulse = elt.tangent_part.total_impulse;
        }
    }
}

macro_rules! specialize_tangents_calculation {
    ($fn_name: ident, $SimdVector: ty, $SimdReal: ty) => {
        #[inline(always)]
        #[cfg(feature = "dim3")]
        pub(crate) fn $fn_name(
            force_dir1: &$SimdVector,
            linvel1: &$SimdVector,
            linvel2: &$SimdVector,
        ) -> [$SimdVector; DIM - 1]
        {
            use na::SimdValue;
            use na::SimdPartialOrd;

            // Compute the tangent direction. Pick the direction of
            // the linear relative velocity, if it is not too small.
            // Otherwise use a fallback direction.
            let relative_linvel = *linvel1 - *linvel2;

            let mut tangent_relative_linvel =
                relative_linvel - *force_dir1 * (force_dir1.gdot(relative_linvel));

            let tangent_linvel_norm = {
                let _disable_fe_except =
                    crate::utils::DisableFloatingPointExceptionsFlags::disable_floating_point_exceptions();
                tangent_relative_linvel.normalize_mut()
            };

            const THRESHOLD: Real = 1.0e-4;
            let use_fallback = tangent_linvel_norm.simd_lt(<$SimdReal>::splat(THRESHOLD));
            let tangent_fallback = force_dir1.orthonormal_vector();

            let tangent1 = tangent_fallback.select(use_fallback, tangent_relative_linvel);
            let bitangent1 = force_dir1.gcross(tangent1);

            [tangent1, bitangent1]
        }
    };
}

specialize_tangents_calculation!(compute_tangent_contact_directions, Vector, Real);
specialize_tangents_calculation!(
    compute_tangent_contact_directions_simd,
    SimdVector,
    SimdReal
);