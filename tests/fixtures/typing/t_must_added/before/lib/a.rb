# typed: true
class A
  extend T::Sig

  def name(id)
    find(id).name
  end
end
