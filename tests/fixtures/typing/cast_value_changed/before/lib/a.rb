# typed: true
class A
  extend T::Sig

  def name(id)
    T.must(find(id)).name
  end
end
