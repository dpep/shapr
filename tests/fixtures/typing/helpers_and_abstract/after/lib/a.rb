# typed: true
class A
  extend T::Sig
  extend T::Helpers
  abstract!

  def total
    1
  end
end
